use std::{collections::HashMap, fs, process::Command};

use serde_json::{Value, json};
use sigstore_crypto::KeyPair;

#[test]
fn inspect_root_command_returns_json_and_a_failure_exit_code_for_invalid_input() {
    let workspace = tempfile::tempdir().expect("temporary workspace must be available");
    let root_path = workspace.path().join("root.json");
    fs::write(&root_path, signed_root()).expect("root fixture must be writable");

    let success = Command::new(env!("CARGO_BIN_EXE_semantic-engine-cli"))
        .args(["context", "channel", "inspect-root", "--root"])
        .arg(&root_path)
        .output()
        .expect("CLI must start");
    assert!(success.status.success(), "{}", String::from_utf8_lossy(&success.stderr));
    let response: Value =
        serde_json::from_slice(&success.stdout).expect("successful output must be JSON");
    assert_eq!(response["version"], 1);
    assert_eq!(response["root_threshold"], 1);
    assert_eq!(response["sha256"].as_str().map(str::len), Some(64));

    let failure = Command::new(env!("CARGO_BIN_EXE_semantic-engine-cli"))
        .args(["context", "channel", "inspect-root", "--root"])
        .arg(workspace.path().join("missing.json"))
        .output()
        .expect("CLI must start for invalid input");
    assert!(!failure.status.success());
    assert!(String::from_utf8_lossy(&failure.stderr).contains("semantic-engine:"));
    assert!(failure.stdout.is_empty());
}

fn signed_root() -> Vec<u8> {
    let keypair = KeyPair::generate_ecdsa_p256().expect("test key must generate");
    let key = json!({
        "keytype": "ecdsa",
        "scheme": "ecdsa-sha2-nistp256",
        "keyval": { "public": keypair.public_key_der().unwrap().to_pem() }
    });
    let parsed = serde_json::from_value::<sigstore_tuf::Key>(key.clone()).unwrap();
    let key_id = parsed.key_id().unwrap();
    let roles = ["root", "targets", "snapshot", "timestamp"]
        .into_iter()
        .map(|role| (role, json!({ "keyids": [key_id], "threshold": 1 })))
        .collect::<HashMap<_, _>>();
    let signed = json!({
        "_type": "root",
        "spec_version": "1.0.0",
        "version": 1,
        "expires": "2030-01-01T00:00:00Z",
        "consistent_snapshot": false,
        "keys": { key_id.clone(): key },
        "roles": roles
    });
    let canonical = sigstore_tuf::canonical_json::to_canonical_bytes(&signed).unwrap();
    let signature = keypair.sign(&canonical).unwrap();
    serde_json::to_vec(&json!({
        "signed": signed,
        "signatures": [{ "keyid": key_id, "sig": hex::encode(signature.as_bytes()) }]
    }))
    .unwrap()
}
