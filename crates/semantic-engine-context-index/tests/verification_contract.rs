use std::{collections::HashMap, fs, path::Path};

use semantic_engine_context_index::{
    RevocationReason, inspect_channel_root, inspect_offline_channel,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sigstore_crypto::KeyPair;
use tempfile::TempDir;

const EXPIRES: &str = "2030-01-01T00:00:00Z";
const NOW: &str = "2026-08-03T00:00:00Z";

#[tokio::test]
async fn verified_tuf_channel_exposes_packages_and_signed_revocations() {
    let fixture = ChannelFixture::new();

    let verified = inspect_offline_channel(
        fixture.root.path(),
        fixture.root.path().join("metadata/root.json"),
        fixture.root.path().join("state"),
        NOW,
    )
    .await
    .unwrap();

    assert_eq!(verified.channel.id, "answer-atlas");
    assert_eq!(verified.channel.name, "Answer Atlas");
    assert_eq!(verified.root_version, 1);
    assert_eq!(verified.timestamp_version, 1);
    assert_eq!(verified.snapshot_version, 1);
    assert_eq!(verified.targets_version, 1);
    assert_eq!(verified.packages.len(), 1);
    assert_eq!(verified.packages[0].metadata.package_version, "0.1.0");
    assert_eq!(verified.packages[0].archive_sha256, fixture.archive_sha256);
    assert_eq!(
        verified.packages[0].revocation.as_ref().unwrap().reason,
        RevocationReason::InvalidData
    );
}

#[test]
fn root_preview_exposes_the_out_of_band_trust_decision() {
    let fixture = ChannelFixture::new();

    let preview =
        inspect_channel_root(fixture.root.path().join("metadata/root.json"), NOW).unwrap();

    assert_eq!(preview.sha256.len(), 64);
    assert_eq!(preview.version, 1);
    assert_eq!(preview.root_threshold, 1);
    assert_eq!(preview.root_key_ids.len(), 1);
    assert!(!preview.consistent_snapshot);
}

#[test]
fn expired_root_is_rejected_before_the_operator_can_approve_it() {
    let fixture = ChannelFixture::new();

    let error = inspect_channel_root(
        fixture.root.path().join("metadata/root.json"),
        "2031-01-01T00:00:00Z",
    )
    .unwrap_err();

    assert!(error.to_string().contains("expired"), "{error}");
}

#[tokio::test]
async fn corrupted_signed_profile_bytes_are_rejected() {
    let fixture = ChannelFixture::new();
    fs::write(fixture.root.path().join("targets/channel-profile.json"), b"{\"tampered\":true}")
        .unwrap();

    let error = inspect_offline_channel(
        fixture.root.path(),
        fixture.root.path().join("metadata/root.json"),
        fixture.root.path().join("state"),
        NOW,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("integrity"), "{error}");
}

#[tokio::test]
async fn metadata_mix_and_match_is_rejected() {
    let fixture = ChannelFixture::new();
    let targets_path = fixture.root.path().join("metadata/targets.json");
    let mut targets = fs::read(&targets_path).unwrap();
    targets.push(b' ');
    fs::write(targets_path, targets).unwrap();

    let error = inspect_offline_channel(
        fixture.root.path(),
        fixture.root.path().join("metadata/root.json"),
        fixture.root.path().join("state"),
        NOW,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("integrity"), "{error}");
}

#[tokio::test]
async fn expired_timestamp_is_rejected() {
    let fixture = ChannelFixture::new();
    fixture.rewrite_timestamp_with_expiry(2, "2025-01-01T00:00:00Z");

    let error = inspect_offline_channel(
        fixture.root.path(),
        fixture.root.path().join("metadata/root.json"),
        fixture.root.path().join("state"),
        NOW,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("expired"), "{error}");
}

#[tokio::test]
async fn persisted_tuf_state_rejects_timestamp_rollback_after_restart() {
    let fixture = ChannelFixture::new();
    let root_path = fixture.root.path().join("metadata/root.json");
    let state_path = fixture.root.path().join("state");
    inspect_offline_channel(fixture.root.path(), &root_path, &state_path, NOW).await.unwrap();

    fixture.rewrite_timestamp(0);
    let error =
        inspect_offline_channel(fixture.root.path(), root_path, state_path, NOW).await.unwrap_err();

    assert!(error.to_string().contains("rollback"), "{error}");
}

#[tokio::test]
async fn observed_package_revocation_remains_a_local_tombstone() {
    let fixture = ChannelFixture::new();
    let root_path = fixture.root.path().join("metadata/root.json");
    let state_path = fixture.root.path().join("state");
    inspect_offline_channel(fixture.root.path(), &root_path, &state_path, NOW).await.unwrap();

    fixture.publish_without_revocations();
    let verified =
        inspect_offline_channel(fixture.root.path(), root_path, state_path, NOW).await.unwrap();

    assert_eq!(verified.revocation_sequence, 2);
    assert_eq!(
        verified.packages[0].revocation.as_ref().unwrap().reason,
        RevocationReason::InvalidData
    );
}

struct ChannelFixture {
    root: TempDir,
    archive_sha256: String,
    keypair: KeyPair,
    key_id: String,
}

impl ChannelFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let metadata_dir = root.path().join("metadata");
        let targets_dir = root.path().join("targets");
        fs::create_dir_all(&metadata_dir).unwrap();
        fs::create_dir_all(targets_dir.join("packages/answer-atlas/core-titles")).unwrap();

        let keypair = KeyPair::generate_ecdsa_p256().unwrap();
        let (key_id, key) = key_entry(&keypair);
        let archive_path = "answer-atlas-core-titles-0.1.0.zip";
        let archive = b"PK fixture context archive";
        let archive_sha256 = hex::encode(Sha256::digest(archive));
        fs::write(targets_dir.join(archive_path), archive).unwrap();

        let channel_profile = serde_json::to_vec(&json!({
            "$schema": "urn:semantic-engine:context-channel-profile:1",
            "formatVersion": 1,
            "id": "answer-atlas",
            "name": "Answer Atlas",
            "homepage": "https://github.com/W-D0n/answer-atlas",
            "packages": [{
                "path": archive_path,
                "metadata": {
                    "profile": "context-target-v1",
                    "packageId": "urn:answer-atlas:pack:core-titles",
                    "packageName": "answer-atlas-core-titles",
                    "packageVersion": "0.1.0",
                    "formatVersion": "0.1.0",
                    "kind": "recognition-context",
                    "locales": ["en", "fr"],
                    "kinds": ["game", "movie"],
                    "targetCount": 84,
                    "spdxLicenseExpression": "CC0-1.0"
                }
            }]
        }))
        .unwrap();
        let revocations = serde_json::to_vec(&json!({
            "$schema": "urn:semantic-engine:context-revocations:1",
            "formatVersion": 1,
            "sequence": 1,
            "updatedAt": "2026-08-02T00:00:00Z",
            "entries": [{
                "archiveSha256": archive_sha256,
                "packageId": "urn:answer-atlas:pack:core-titles",
                "packageVersion": "0.1.0",
                "effectiveAt": "2026-08-02T00:00:00Z",
                "reason": "invalid-data",
                "replacement": "0.1.1"
            }]
        }))
        .unwrap();
        fs::write(targets_dir.join("channel-profile.json"), &channel_profile).unwrap();
        fs::write(targets_dir.join("revocations-v1.json"), &revocations).unwrap();

        let delegated_signed = json!({
            "_type": "targets",
            "spec_version": "1.0.0",
            "version": 1,
            "expires": EXPIRES,
            "targets": {
                archive_path: target(archive, None)
            }
        });
        let delegated = envelope(&delegated_signed, &key_id, &keypair);
        fs::write(metadata_dir.join("answer-atlas.json"), &delegated).unwrap();

        let targets_signed = json!({
            "_type": "targets",
            "spec_version": "1.0.0",
            "version": 1,
            "expires": EXPIRES,
            "targets": {
                "channel-profile.json": target(&channel_profile, None),
                "revocations-v1.json": target(&revocations, None)
            },
            "delegations": {
                "keys": { key_id.clone(): key.clone() },
                "roles": [{
                    "name": "answer-atlas",
                    "keyids": [key_id],
                    "threshold": 1,
                    "terminating": true,
                    "paths": [archive_path]
                }]
            }
        });
        let targets = envelope(&targets_signed, &key_id, &keypair);
        fs::write(metadata_dir.join("targets.json"), &targets).unwrap();

        let snapshot_signed = json!({
            "_type": "snapshot",
            "spec_version": "1.0.0",
            "version": 1,
            "expires": EXPIRES,
            "meta": {
                "targets.json": metafile(&targets, 1),
                "answer-atlas.json": metafile(&delegated, 1)
            }
        });
        let snapshot = envelope(&snapshot_signed, &key_id, &keypair);
        fs::write(metadata_dir.join("snapshot.json"), &snapshot).unwrap();

        let timestamp_signed = json!({
            "_type": "timestamp",
            "spec_version": "1.0.0",
            "version": 1,
            "expires": EXPIRES,
            "meta": { "snapshot.json": metafile(&snapshot, 1) }
        });
        let timestamp = envelope(&timestamp_signed, &key_id, &keypair);
        fs::write(metadata_dir.join("timestamp.json"), timestamp).unwrap();

        let roles = ["root", "targets", "snapshot", "timestamp"]
            .into_iter()
            .map(|role| (role, json!({ "keyids": [key_id], "threshold": 1 })))
            .collect::<HashMap<_, _>>();
        let root_signed = json!({
            "_type": "root",
            "spec_version": "1.0.0",
            "version": 1,
            "expires": EXPIRES,
            "consistent_snapshot": false,
            "keys": { key_id.clone(): key },
            "roles": roles
        });
        let root_bytes = envelope(&root_signed, &key_id, &keypair);
        fs::write(metadata_dir.join("1.root.json"), &root_bytes).unwrap();
        fs::write(metadata_dir.join("root.json"), root_bytes).unwrap();

        Self { root, archive_sha256, keypair, key_id }
    }

    fn rewrite_timestamp(&self, version: u64) {
        self.rewrite_timestamp_with_expiry(version, EXPIRES);
    }

    fn rewrite_timestamp_with_expiry(&self, version: u64, expires: &str) {
        let snapshot = fs::read(self.root.path().join("metadata/snapshot.json")).unwrap();
        let signed = json!({
            "_type": "timestamp",
            "spec_version": "1.0.0",
            "version": version,
            "expires": expires,
            "meta": { "snapshot.json": metafile(&snapshot, 1) }
        });
        fs::write(
            self.root.path().join("metadata/timestamp.json"),
            envelope(&signed, &self.key_id, &self.keypair),
        )
        .unwrap();
    }

    fn publish_without_revocations(&self) {
        let targets_dir = self.root.path().join("targets");
        let metadata_dir = self.root.path().join("metadata");
        let archive_path = "answer-atlas-core-titles-0.1.0.zip";
        let archive = fs::read(targets_dir.join(archive_path)).unwrap();
        let channel_profile = fs::read(targets_dir.join("channel-profile.json")).unwrap();
        let revocations = serde_json::to_vec(&json!({
            "$schema": "urn:semantic-engine:context-revocations:1",
            "formatVersion": 1,
            "sequence": 2,
            "updatedAt": "2026-08-03T00:00:00Z",
            "entries": []
        }))
        .unwrap();
        fs::write(targets_dir.join("revocations-v1.json"), &revocations).unwrap();
        let (_, key) = key_entry(&self.keypair);
        let delegated_signed = json!({
            "_type": "targets",
            "spec_version": "1.0.0",
            "version": 2,
            "expires": EXPIRES,
            "targets": { archive_path: target(&archive, None) }
        });
        let delegated = envelope(&delegated_signed, &self.key_id, &self.keypair);
        fs::write(metadata_dir.join("answer-atlas.json"), &delegated).unwrap();
        let targets_signed = json!({
            "_type": "targets",
            "spec_version": "1.0.0",
            "version": 2,
            "expires": EXPIRES,
            "targets": {
                "channel-profile.json": target(&channel_profile, None),
                "revocations-v1.json": target(&revocations, None)
            },
            "delegations": {
                "keys": { self.key_id.clone(): key },
                "roles": [{
                    "name": "answer-atlas",
                    "keyids": [self.key_id],
                    "threshold": 1,
                    "terminating": true,
                    "paths": [archive_path]
                }]
            }
        });
        let targets = envelope(&targets_signed, &self.key_id, &self.keypair);
        fs::write(metadata_dir.join("targets.json"), &targets).unwrap();
        let snapshot_signed = json!({
            "_type": "snapshot",
            "spec_version": "1.0.0",
            "version": 2,
            "expires": EXPIRES,
            "meta": {
                "targets.json": metafile(&targets, 2),
                "answer-atlas.json": metafile(&delegated, 2)
            }
        });
        let snapshot = envelope(&snapshot_signed, &self.key_id, &self.keypair);
        fs::write(metadata_dir.join("snapshot.json"), &snapshot).unwrap();
        let timestamp_signed = json!({
            "_type": "timestamp",
            "spec_version": "1.0.0",
            "version": 2,
            "expires": EXPIRES,
            "meta": { "snapshot.json": metafile(&snapshot, 2) }
        });
        fs::write(
            metadata_dir.join("timestamp.json"),
            envelope(&timestamp_signed, &self.key_id, &self.keypair),
        )
        .unwrap();
    }
}

fn key_entry(keypair: &KeyPair) -> (String, Value) {
    let key = json!({
        "keytype": "ecdsa",
        "scheme": "ecdsa-sha2-nistp256",
        "keyval": { "public": keypair.public_key_der().unwrap().to_pem() }
    });
    let parsed = serde_json::from_value::<sigstore_tuf::Key>(key.clone()).unwrap();
    (parsed.key_id().unwrap(), key)
}

fn target(bytes: &[u8], custom: Option<Value>) -> Value {
    let mut value = json!({
        "length": bytes.len(),
        "hashes": { "sha256": hex::encode(Sha256::digest(bytes)) }
    });
    if let Some(custom) = custom {
        value["custom"] = custom;
    }
    value
}

fn metafile(bytes: &[u8], version: u64) -> Value {
    json!({
        "version": version,
        "length": bytes.len(),
        "hashes": { "sha256": hex::encode(Sha256::digest(bytes)) }
    })
}

fn envelope(signed: &Value, key_id: &str, keypair: &KeyPair) -> Vec<u8> {
    let canonical = sigstore_tuf::canonical_json::to_canonical_bytes(signed).unwrap();
    let signature = keypair.sign(&canonical).unwrap();
    serde_json::to_vec(&json!({
        "signed": signed,
        "signatures": [{ "keyid": key_id, "sig": hex::encode(signature.as_bytes()) }]
    }))
    .unwrap()
}

#[allow(dead_code)]
fn assert_exists(path: &Path) {
    assert!(path.exists(), "{} should exist", path.display());
}
