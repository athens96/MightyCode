//! MC-33: a workspace on an SSH machine is approved, saved and loaded with its machine and remote folder; one whose names are not ones
//! the store keeps is never approved, and a saved one that fails the rules is dropped by the load.

mod common;

use std::fs;

use common::temp_dir;
use mighty_core::locale::Locale;
use mighty_core::model::SshWorkspaceRef;
use mighty_core::normalize::valid_workspace;
use mighty_core::store::Store;

#[test]
fn an_ssh_workspace_is_approved_saved_and_loaded() {
    let dir = temp_dir("ssh-workspace");
    let mut store = Store::new(dir.clone(), None, true);
    let mut state = store.load(&Locale::new("en")).unwrap();
    let local = dir.join("ssh").join("workspaces");
    let stand_in = |id: &str| local.join(id).to_string_lossy().into_owned();
    let machine = "0123456789abcdef0123456789abcdef";
    let workspace = store.approve_ssh(machine, "devbox", "~/src/my app", stand_in).unwrap();
    assert!(workspace.is_ssh() && !workspace.is_local());
    assert_eq!(workspace.name(), "my app");
    assert_eq!(workspace.ssh, Some(SshWorkspaceRef { machine: machine.into(), dir: "~/src/my app".into(), host: "devbox".into() }));
    assert!(valid_workspace(&workspace));
    // The same machine and folder again is the same workspace.
    let again = store.approve_ssh(machine, "devbox", "~/src/my app", |_| unreachable!()).unwrap();
    assert_eq!(again.id(), workspace.id());

    state.workspaces_mut().push(workspace.clone());
    store.save(&state).unwrap();
    let mut reloaded = Store::new(dir.clone(), None, true);
    let loaded = reloaded.load(&Locale::new("en")).unwrap();
    let back = loaded.workspace(workspace.id()).unwrap();
    assert_eq!(back.ssh, workspace.ssh);
    assert_eq!(back.path, workspace.path);
    assert!(reloaded.approves(back));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn names_the_store_does_not_keep_are_refused() {
    let dir = temp_dir("ssh-refused");
    let mut store = Store::new(dir.clone(), None, true);
    store.load(&Locale::new("en")).unwrap();
    let stand_in = |id: &str| dir.join(id).to_string_lossy().into_owned();
    let machine = "0123456789abcdef0123456789abcdef";
    assert!(store.approve_ssh("not an id", "devbox", "/srv", stand_in).is_none());
    assert!(store.approve_ssh(machine, "devbox", "relative/dir", stand_in).is_none());
    assert!(store.approve_ssh(machine, "devbox", "/a\nb", stand_in).is_none());
    assert!(store.approve_ssh(machine, "  ", "/srv", stand_in).is_none());
    assert!(store.approve_ssh(machine, "devbox", "/srv", |_| "relative".into()).is_none());

    let mut workspace = store.approve_ssh(machine, "devbox", "/srv", stand_in).unwrap();
    workspace.ssh.as_mut().unwrap().dir = "srv".into();
    assert!(!valid_workspace(&workspace), "a saved ssh workspace with a bad folder is dropped by the load");
    fs::remove_dir_all(dir).unwrap();
}
