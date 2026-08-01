use std::{process, time::SystemTime};

use semantic_engine_credential_vault::{CredentialVault, OsCredentialVault, VaultError};

struct Cleanup<'a> {
    vault: &'a OsCredentialVault,
    credential_id: String,
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        let _ = self.vault.delete(&self.credential_id);
    }
}

#[test]
#[ignore = "writes and immediately removes a short-lived OS credential"]
fn native_vault_round_trip_does_not_use_the_project_database() {
    let vault = OsCredentialVault::semantic_engine().expect("native vault should be available");
    let nonce = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
    let credential_id = format!("conformance-{}-{nonce}", process::id());
    let _cleanup = Cleanup { vault: &vault, credential_id: credential_id.clone() };

    vault.store(&credential_id, b"short-lived-conformance-secret").unwrap();
    assert_eq!(vault.load(&credential_id).unwrap().expose(), b"short-lived-conformance-secret");
    vault.delete(&credential_id).unwrap();
    assert!(matches!(vault.load(&credential_id), Err(VaultError::Missing)));
}
