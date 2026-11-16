//! The phone's pairing files both ways against the Swift app's own code (mac-plan.md M-B "Done when"): what `tools/parity-mac pairing
//! write` made with `MobileRemoteService` and `MobileDeviceRegistry` reads here as Swift says it holds, unchanged; and what this writes
//! is left for `parity-mac pairing read`, which reads it with the same Swift code.
//!
//! The folders are under `$MIGHTY_PARITY_MAC_DIR` (the Mac CI job); without it the test says so and passes, unless
//! `MIGHTY_PARITY_MAC=require`.

use std::path::PathBuf;

use base64::Engine;
use mighty_platform::pairing::four_files::{FourFiles, PlainFiles, swift_timestamp};
use mighty_platform::{PairedDevice, PairingIdentity, PairingStore, PushRegistration};
use serde_json::{Value, json};

const FILES: [&str; 5] = ["host-id", "mobile-remote.key", "relay-host-token.json", "relay-keypair.json", "devices.json"];

fn parity_dir() -> Option<PathBuf> {
    match std::env::var_os("MIGHTY_PARITY_MAC_DIR") {
        Some(dir) => Some(PathBuf::from(dir)),
        None => {
            assert_ne!(
                std::env::var("MIGHTY_PARITY_MAC").as_deref(),
                Ok("require"),
                "MIGHTY_PARITY_MAC=require without MIGHTY_PARITY_MAC_DIR"
            );
            None
        }
    }
}

fn public_key(secret: &[u8; 32]) -> String {
    base64::engine::general_purpose::STANDARD.encode(x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(*secret)).to_bytes())
}

/// A row as Swift's `JSONEncoder` encodes `MobileDevice` (nil left out).
fn device_json(device: &PairedDevice) -> Value {
    let mut row = json!({"id": device.id, "name": device.name, "firstSeen": swift_timestamp(&device.first_seen), "lastSeen": swift_timestamp(&device.last_seen)});
    if let Some(hash) = &device.token_hash {
        row["tokenHash"] = json!(hash);
    }
    if let Some(push) = &device.push {
        row["push"] = json!({"handle": push.handle, "kinds": push.kinds, "updatedAt": swift_timestamp(&push.updated_at)});
    }
    row
}

#[test]
fn swift_written_pairing_files_read_here_and_rust_written_ones_are_left_for_swift() {
    let Some(dir) = parity_dir() else {
        eprintln!("no Swift pairing files (MIGHTY_PARITY_MAC_DIR): skipped; the Mac CI job runs it");
        return;
    };
    // Swift's files, read here.
    let swift_folder = dir.join("pairing-swift");
    let expected: Value = serde_json::from_slice(&std::fs::read(dir.join("pairing-swift.json")).unwrap()).unwrap();
    let before: Vec<Option<Vec<u8>>> = FILES.iter().map(|name| std::fs::read(swift_folder.join(name)).ok()).collect();
    let read = FourFiles::new(swift_folder.clone(), Box::new(PlainFiles)).load().unwrap().expect("Swift's files hold an identity");
    let after: Vec<Option<Vec<u8>>> = FILES.iter().map(|name| std::fs::read(swift_folder.join(name)).ok()).collect();
    assert_eq!(before, after, "reading Swift's files changed one (something read as damaged)");
    assert_eq!(read.host_id, expected["hostId"].as_str().unwrap());
    assert_eq!(read.pairing_key, expected["pairingKey"].as_str().unwrap());
    assert_eq!(read.host_token, expected["hostToken"].as_str().unwrap());
    assert_eq!(public_key(&read.secret_key), expected["publicKeyB64"].as_str().unwrap(), "the relay key pair's secret");
    let devices: Vec<Value> = read.devices.iter().map(device_json).collect();
    assert_eq!(Value::Array(devices), expected["devices"], "the phones Swift wrote");
    assert!(read.devices.iter().any(|d| d.push.is_some()) && read.devices.iter().any(|d| d.id == "legacy"));

    // Rust's files, for Swift to read.
    let identity = PairingIdentity {
        host_id: "0b9d7f1e-5c3a-4e2b-9a8f-1c2d3e4f5a6b".into(),
        host_token: "0123456789abcdef".repeat(4),
        pairing_key: "AQgPFh0kKzI5QEdOVVxjanF4f4aNlJuiqbC3vsXM09o".into(),
        secret_key: [0x5a; 32],
        devices: vec![
            PairedDevice {
                id: "legacy".into(),
                name: "Legacy app".into(),
                token_hash: None,
                first_seen: "2026-10-23T01:02:03.000Z".into(),
                last_seen: "2026-10-23T01:02:03.000Z".into(),
                push: None,
            },
            PairedDevice {
                id: "cGhvbmUtdHdvLTAwMDAwMDA".into(),
                name: "민수의 \"iPhone\" / 😀".into(),
                token_hash: Some("5723c1b7256eba050781d42f7d4fd96201c4b75d7673b1a82ffbcba6dc2e1c23".into()),
                first_seen: "2026-10-23T01:02:03.0000000+00:00".into(),
                last_seen: "2026-11-01T05:06:07.1234567+00:00".into(),
                push: Some(PushRegistration {
                    handle: "__79_Pv6-fj39vX08_Lx8O_u7ezr6uno5-bl5OPi4eA".into(),
                    kinds: vec!["completed".into(), "question".into()],
                    updated_at: "2026-11-01T05:06:07.1234567+00:00".into(),
                }),
            },
        ],
    };
    let rust_folder = dir.join("pairing-rust");
    let _ = std::fs::remove_dir_all(&rust_folder);
    FourFiles::new(rust_folder, Box::new(PlainFiles)).save(&identity).unwrap();
    let want = json!({
        "pairingKey": identity.pairing_key,
        "hostToken": identity.host_token,
        "publicKeyB64": public_key(&identity.secret_key),
        "devices": identity.devices.iter().map(device_json).collect::<Vec<_>>(),
    });
    std::fs::write(dir.join("pairing-rust.json"), serde_json::to_vec(&want).unwrap()).unwrap();
}
