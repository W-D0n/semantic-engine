//! Safe discovery of Semantic Engine context packages through standard TUF channels.

use std::{
    collections::{BTreeMap, HashSet},
    fs, io,
    path::{Component, Path, PathBuf},
};

use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sigstore_tuf::{FileStore, Updater, UpdaterConfig, transport::FetchFuture};
use spdx::Expression;

const CHANNEL_PROFILE_TARGET: &str = "channel-profile.json";
const REVOCATIONS_TARGET: &str = "revocations-v1.json";
const MAX_ROOT_BYTES: u64 = 512_000;
const MAX_PACKAGES: usize = 10_000;
const MAX_REVOCATIONS: usize = 10_000;
const MAX_REVOCATION_TOMBSTONES: usize = 50_000;
const MAX_REVOCATION_STATE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_TEXT_CHARS: usize = 512;
const MAX_URL_CHARS: usize = 2_048;
const MAX_CONTEXT_ARCHIVE_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("channel I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("TUF verification failed: {0}")]
    Tuf(#[from] sigstore_tuf::Error),
    #[error("invalid channel JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid context channel: {0}")]
    Invalid(String),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ChannelProfile {
    #[serde(rename = "$schema")]
    pub schema: String,
    pub format_version: u32,
    pub id: String,
    pub name: String,
    pub homepage: String,
    pub packages: Vec<ChannelPackageReference>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ChannelPackageReference {
    pub path: String,
    pub metadata: ContextTargetMetadata,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ContextTargetMetadata {
    pub profile: String,
    pub package_id: String,
    pub package_name: String,
    pub package_version: String,
    pub format_version: String,
    pub kind: String,
    pub locales: Vec<String>,
    pub kinds: Vec<String>,
    pub target_count: usize,
    pub spdx_license_expression: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RevocationReason {
    Compromised,
    InvalidData,
    Legal,
    Withdrawn,
    Other,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ContextRevocation {
    pub archive_sha256: String,
    pub package_id: String,
    pub package_version: String,
    pub effective_at: String,
    pub reason: RevocationReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RevocationsDocument {
    #[serde(rename = "$schema")]
    schema: String,
    format_version: u32,
    sequence: u64,
    updated_at: String,
    entries: Vec<ContextRevocation>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RevocationState {
    format_version: u32,
    highest_sequence: u64,
    document_sha256: String,
    tombstones: BTreeMap<String, ContextRevocation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ContextChannelPackage {
    pub target_path: String,
    pub archive_length: u64,
    pub archive_sha256: String,
    pub metadata: ContextTargetMetadata,
    pub revocation: Option<ContextRevocation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VerifiedContextChannel {
    pub channel: ChannelProfile,
    pub root_sha256: String,
    pub root_version: u64,
    pub timestamp_version: u64,
    pub snapshot_version: u64,
    pub targets_version: u64,
    pub timestamp_expires: String,
    pub verified_at: String,
    pub revocation_sequence: u64,
    pub revocations_updated_at: String,
    pub packages: Vec<ContextChannelPackage>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ChannelRootPreview {
    pub sha256: String,
    pub version: u64,
    pub expires: String,
    pub consistent_snapshot: bool,
    pub root_threshold: usize,
    pub root_key_ids: Vec<String>,
}

/// Inspect and self-verify a candidate TUF root before the operator approves it.
///
/// Self-signature proves internal consistency, not publisher identity. The
/// returned SHA-256 is the value that must be compared out of band.
pub fn inspect_channel_root(
    root_path: impl AsRef<Path>,
    now: &str,
) -> Result<ChannelRootPreview, ChannelError> {
    let root_bytes = read_bounded_file(root_path.as_ref(), MAX_ROOT_BYTES)?;
    let now =
        now.parse::<jiff::Timestamp>().map_err(|_| invalid("verification time is not RFC 3339"))?;
    let trusted = sigstore_tuf::TrustedMetadataSet::from_root(&root_bytes)?;
    trusted.check_root_expired(now)?;
    let root = trusted.root();
    let binding =
        root.role("root").ok_or_else(|| invalid("root metadata does not define its root role"))?;
    if binding.threshold == 0 || binding.keyids.is_empty() {
        return Err(invalid("root role has no usable signature policy"));
    }
    let mut root_key_ids = binding.keyids.clone();
    root_key_ids.sort();
    root_key_ids.dedup();
    Ok(ChannelRootPreview {
        sha256: format!("{:x}", Sha256::digest(&root_bytes)),
        version: root.version,
        expires: root.expires.clone(),
        consistent_snapshot: root.consistent_snapshot,
        root_threshold: binding.threshold,
        root_key_ids,
    })
}

pub fn inspect_channel_root_now(
    root_path: impl AsRef<Path>,
) -> Result<ChannelRootPreview, ChannelError> {
    inspect_channel_root(root_path, &jiff::Timestamp::now().to_string())
}

/// Verify an offline TUF repository and return its Semantic Engine package index.
///
/// `trusted_root_path` is the out-of-band trust anchor selected by the caller.
/// `state_directory` persists verified TUF metadata so version rollback checks
/// survive process restarts. No URL or executable content from the channel is
/// opened by this operation.
pub async fn inspect_offline_channel(
    channel_directory: impl AsRef<Path>,
    trusted_root_path: impl AsRef<Path>,
    state_directory: impl AsRef<Path>,
    now: &str,
) -> Result<VerifiedContextChannel, ChannelError> {
    let channel_directory = channel_directory.as_ref().canonicalize()?;
    if !channel_directory.is_dir() {
        return Err(invalid("channel path is not a directory"));
    }
    let root_bytes = read_bounded_file(trusted_root_path.as_ref(), MAX_ROOT_BYTES)?;
    let root_sha256 = format!("{:x}", Sha256::digest(&root_bytes));
    let now =
        now.parse::<jiff::Timestamp>().map_err(|_| invalid("verification time is not RFC 3339"))?;

    let repository = DirectoryRepository::new(&channel_directory)?;
    let state_directory = state_directory.as_ref();
    fs::create_dir_all(state_directory)?;
    let cache = FileStore::new(state_directory.join("tuf-cache"));
    let config = UpdaterConfig {
        root_max_length: MAX_ROOT_BYTES,
        timestamp_max_length: 64 * 1024,
        snapshot_max_length: 2 * 1024 * 1024,
        targets_max_length: 8 * 1024 * 1024,
        target_max_length: MAX_CONTEXT_ARCHIVE_BYTES,
        max_root_rotations: 32,
        max_delegations: 32,
    };
    let mut updater = Updater::new(repository, &root_bytes)?.with_config(config).with_store(cache);
    updater.refresh(now).await?;

    let root_version = updater.trusted().root().version;
    let timestamp = updater
        .trusted()
        .timestamp()
        .ok_or_else(|| invalid("verified timestamp metadata is missing"))?;
    let timestamp_version = timestamp.version;
    let timestamp_expires = timestamp.expires.clone();
    let snapshot_version = updater
        .trusted()
        .snapshot()
        .ok_or_else(|| invalid("verified snapshot metadata is missing"))?
        .version;
    let targets = updater
        .trusted()
        .targets()
        .ok_or_else(|| invalid("verified targets metadata is missing"))?;
    let targets_version = targets.version;
    let target_entries = targets.targets.clone();
    if target_entries.len() > MAX_PACKAGES + 2 {
        return Err(invalid("channel exposes too many top-level targets"));
    }

    require_target(&target_entries, CHANNEL_PROFILE_TARGET)?;
    require_target(&target_entries, REVOCATIONS_TARGET)?;
    let channel_bytes = updater.get_target(CHANNEL_PROFILE_TARGET, now).await?;
    let revocation_bytes = updater.get_target(REVOCATIONS_TARGET, now).await?;
    let channel: ChannelProfile = serde_json::from_slice(&channel_bytes)?;
    let revocations: RevocationsDocument = serde_json::from_slice(&revocation_bytes)?;
    validate_channel_profile(&channel)?;
    validate_revocations(&revocations)?;

    let (revocation_state, revocation_state_changed) =
        prepare_revocation_state(state_directory, &revocations, &revocation_bytes)?;
    let mut packages = Vec::with_capacity(channel.packages.len());
    let mut identities = HashSet::new();
    for package in &channel.packages {
        let target_path = &package.path;
        let target = updater
            .get_targetinfo(target_path, now)
            .await?
            .ok_or_else(|| invalid(format!("listed context target {target_path:?} is missing")))?;
        let metadata = package.metadata.clone();
        validate_package(target_path, &metadata, target.length, &target.hashes)?;
        let archive_sha256 =
            target.hashes.get("sha256").expect("validated SHA-256 is present").to_ascii_lowercase();
        if !identities.insert((metadata.package_id.clone(), metadata.package_version.clone())) {
            return Err(invalid("duplicate package identity and version"));
        }
        let revocation = revocation_state.tombstones.get(&archive_sha256).cloned();
        if let Some(revocation) = &revocation
            && (revocation.package_id != metadata.package_id
                || revocation.package_version != metadata.package_version)
        {
            return Err(invalid("revocation identity conflicts with signed target metadata"));
        }
        packages.push(ContextChannelPackage {
            target_path: target_path.clone(),
            archive_length: target.length,
            archive_sha256,
            metadata,
            revocation,
        });
    }
    if packages.is_empty() {
        return Err(invalid("channel does not expose any Semantic Engine package"));
    }
    packages.sort_by(|left, right| left.target_path.cmp(&right.target_path));
    if revocation_state_changed {
        write_revocation_state(state_directory, &revocation_state)?;
    }

    Ok(VerifiedContextChannel {
        channel,
        root_sha256,
        root_version,
        timestamp_version,
        snapshot_version,
        targets_version,
        timestamp_expires,
        verified_at: now.to_string(),
        revocation_sequence: revocations.sequence,
        revocations_updated_at: revocations.updated_at,
        packages,
    })
}

pub async fn inspect_offline_channel_now(
    channel_directory: impl AsRef<Path>,
    trusted_root_path: impl AsRef<Path>,
    state_directory: impl AsRef<Path>,
) -> Result<VerifiedContextChannel, ChannelError> {
    inspect_offline_channel(
        channel_directory,
        trusted_root_path,
        state_directory,
        &jiff::Timestamp::now().to_string(),
    )
    .await
}

fn prepare_revocation_state(
    state_directory: &Path,
    document: &RevocationsDocument,
    document_bytes: &[u8],
) -> Result<(RevocationState, bool), ChannelError> {
    let path = state_directory.join("revocation-state.json");
    let document_sha256 = format!("{:x}", Sha256::digest(document_bytes));
    let mut state = match fs::metadata(&path) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.len() > MAX_REVOCATION_STATE_BYTES {
                return Err(invalid("local revocation state is not regular or is too large"));
            }
            let bytes = fs::read(&path)?;
            if bytes.len() as u64 > MAX_REVOCATION_STATE_BYTES {
                return Err(invalid("local revocation state grew beyond its size limit"));
            }
            let state: RevocationState = serde_json::from_slice(&bytes)?;
            validate_revocation_state(&state)?;
            state
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => RevocationState {
            format_version: 1,
            highest_sequence: 0,
            document_sha256: String::new(),
            tombstones: BTreeMap::new(),
        },
        Err(error) => return Err(error.into()),
    };

    if document.sequence < state.highest_sequence {
        return Err(invalid("revocation sequence rollback detected"));
    }
    if document.sequence == state.highest_sequence {
        if state.document_sha256 != document_sha256 {
            return Err(invalid("revocation sequence equivocation detected"));
        }
        return Ok((state, false));
    }
    for entry in &document.entries {
        state
            .tombstones
            .entry(entry.archive_sha256.to_ascii_lowercase())
            .or_insert_with(|| entry.clone());
    }
    if state.tombstones.len() > MAX_REVOCATION_TOMBSTONES {
        return Err(invalid("local revocation tombstone limit exceeded"));
    }
    state.highest_sequence = document.sequence;
    state.document_sha256 = document_sha256;
    Ok((state, true))
}

fn validate_revocation_state(state: &RevocationState) -> Result<(), ChannelError> {
    if state.format_version != 1
        || state.highest_sequence == 0
        || state.tombstones.len() > MAX_REVOCATION_TOMBSTONES
    {
        return Err(invalid("unsupported local revocation state"));
    }
    validate_sha256(&state.document_sha256)?;
    for (hash, entry) in &state.tombstones {
        validate_sha256(hash)?;
        if hash != &entry.archive_sha256.to_ascii_lowercase() {
            return Err(invalid("local revocation tombstone hash conflicts with its entry"));
        }
    }
    Ok(())
}

fn write_revocation_state(path: &Path, state: &RevocationState) -> Result<(), ChannelError> {
    let bytes = serde_json::to_vec_pretty(state)?;
    if bytes.len() as u64 > MAX_REVOCATION_STATE_BYTES {
        return Err(invalid("local revocation state exceeds its size limit"));
    }
    let mut temporary = tempfile::NamedTempFile::new_in(path)?;
    use io::Write as _;
    temporary.write_all(&bytes)?;
    temporary
        .persist(path.join("revocation-state.json"))
        .map_err(|error| ChannelError::Io(error.error))?;
    Ok(())
}

fn require_target(
    targets: &BTreeMap<String, sigstore_tuf::TargetFile>,
    name: &str,
) -> Result<(), ChannelError> {
    if targets.contains_key(name) {
        Ok(())
    } else {
        Err(invalid(format!("required signed target {name:?} is missing")))
    }
}

fn validate_channel_profile(profile: &ChannelProfile) -> Result<(), ChannelError> {
    if profile.schema != "urn:semantic-engine:context-channel-profile:1"
        || profile.format_version != 1
    {
        return Err(invalid("unsupported channel profile"));
    }
    validate_identifier(&profile.id, "channel id")?;
    validate_text(&profile.name, "channel name", MAX_TEXT_CHARS)?;
    validate_https_url(&profile.homepage, "channel homepage")?;
    if profile.packages.is_empty() || profile.packages.len() > MAX_PACKAGES {
        return Err(invalid("channel package list is outside supported limits"));
    }
    let mut paths = HashSet::new();
    for package in &profile.packages {
        let path = &package.path;
        if !paths.insert(path) {
            return Err(invalid("channel package list contains a duplicate path"));
        }
        if !path.ends_with(".zip") || path.contains('\\') {
            return Err(invalid("channel package path must be a portable relative ZIP path"));
        }
        validate_portable_relative_path(path)?;
    }
    Ok(())
}

fn validate_package(
    path: &str,
    metadata: &ContextTargetMetadata,
    archive_length: u64,
    hashes: &BTreeMap<String, String>,
) -> Result<(), ChannelError> {
    if !path.ends_with(".zip") || path.contains('\\') {
        return Err(invalid("context target path must be a portable relative ZIP path"));
    }
    validate_portable_relative_path(path)?;
    if metadata.profile != "context-target-v1"
        || metadata.kind != "recognition-context"
        || metadata.format_version != "0.1.0"
    {
        return Err(invalid("unsupported Semantic Engine target profile"));
    }
    validate_text(&metadata.package_id, "package id", MAX_TEXT_CHARS)?;
    validate_identifier(&metadata.package_name, "package name")?;
    Version::parse(&metadata.package_version)
        .map_err(|_| invalid("package version is not Semantic Versioning 2.0.0"))?;
    Expression::parse(&metadata.spdx_license_expression)
        .map_err(|_| invalid("package license is not a valid SPDX expression"))?;
    if archive_length == 0 || archive_length > MAX_CONTEXT_ARCHIVE_BYTES {
        return Err(invalid("context archive length is outside supported limits"));
    }
    let sha256 = hashes
        .get("sha256")
        .ok_or_else(|| invalid("context archive must be pinned with SHA-256"))?;
    validate_sha256(sha256)?;
    if metadata.locales.is_empty()
        || metadata.locales.len() > 32
        || metadata.kinds.is_empty()
        || metadata.kinds.len() > 3
        || metadata.target_count == 0
        || metadata.target_count > 50_000
    {
        return Err(invalid("context target metadata is outside supported limits"));
    }
    let mut locales = HashSet::new();
    for locale in &metadata.locales {
        if !locales.insert(locale) || !valid_locale(locale) {
            return Err(invalid("context locales must be unique portable BCP 47 tags"));
        }
    }
    let mut kinds = HashSet::new();
    for kind in &metadata.kinds {
        if !kinds.insert(kind) || !matches!(kind.as_str(), "game" | "movie" | "other") {
            return Err(invalid("context kinds must be unique supported values"));
        }
    }
    Ok(())
}

fn validate_revocations(document: &RevocationsDocument) -> Result<(), ChannelError> {
    if document.schema != "urn:semantic-engine:context-revocations:1"
        || document.format_version != 1
        || document.sequence == 0
        || document.entries.len() > MAX_REVOCATIONS
        || document.updated_at.parse::<jiff::Timestamp>().is_err()
    {
        return Err(invalid("unsupported or invalid revocation document"));
    }
    let mut hashes = HashSet::new();
    for entry in &document.entries {
        validate_sha256(&entry.archive_sha256)?;
        if !hashes.insert(entry.archive_sha256.to_ascii_lowercase()) {
            return Err(invalid("duplicate archive revocation"));
        }
        validate_text(&entry.package_id, "revoked package id", MAX_TEXT_CHARS)?;
        Version::parse(&entry.package_version)
            .map_err(|_| invalid("revoked package version is not SemVer"))?;
        if entry.effective_at.parse::<jiff::Timestamp>().is_err() {
            return Err(invalid("revocation effective time is not RFC 3339"));
        }
        if let Some(replacement) = &entry.replacement {
            Version::parse(replacement)
                .map_err(|_| invalid("revocation replacement is not SemVer"))?;
        }
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<(), ChannelError> {
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
    {
        return Err(invalid(format!("{label} is not a portable identifier")));
    }
    Ok(())
}

fn validate_text(value: &str, label: &str, maximum: usize) -> Result<(), ChannelError> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().count() > maximum
        || value.chars().any(char::is_control)
    {
        return Err(invalid(format!("{label} is outside supported limits")));
    }
    Ok(())
}

fn validate_https_url(value: &str, label: &str) -> Result<(), ChannelError> {
    validate_text(value, label, MAX_URL_CHARS)?;
    if !value.starts_with("https://") || value.contains(['\\', '\r', '\n']) {
        return Err(invalid(format!("{label} must be an HTTPS URL")));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), ChannelError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid("SHA-256 must contain exactly 64 hexadecimal characters"));
    }
    Ok(())
}

fn valid_locale(value: &str) -> bool {
    let mut parts = value.split('-');
    let Some(language) = parts.next() else { return false };
    if !(2..=3).contains(&language.len()) || !language.bytes().all(|byte| byte.is_ascii_lowercase())
    {
        return false;
    }
    match (parts.next(), parts.next()) {
        (None, None) => true,
        (Some(region), None) => {
            region.len() == 2 && region.bytes().all(|byte| byte.is_ascii_uppercase())
        }
        _ => false,
    }
}

fn validate_portable_relative_path(value: &str) -> Result<(), ChannelError> {
    let path = Path::new(value);
    if value.len() > 1_024
        || path.is_absolute()
        || path.components().any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid("target path is not a portable relative path"));
    }
    for segment in value.split('/') {
        if segment.is_empty()
            || segment.len() > 128
            || !segment.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        {
            return Err(invalid("target path contains an unsupported segment"));
        }
    }
    Ok(())
}

fn read_bounded_file(path: &Path, maximum: u64) -> Result<Vec<u8>, ChannelError> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err(invalid("trusted root is absent, not regular, or too large"));
    }
    let bytes = fs::read(path)?;
    if bytes.len() as u64 > maximum {
        return Err(invalid("trusted root grew beyond its size limit"));
    }
    Ok(bytes)
}

fn invalid(message: impl Into<String>) -> ChannelError {
    ChannelError::Invalid(message.into())
}

#[derive(Clone, Debug)]
struct DirectoryRepository {
    metadata: PathBuf,
    targets: PathBuf,
}

impl DirectoryRepository {
    fn new(channel: &Path) -> Result<Self, ChannelError> {
        let metadata = channel.join("metadata").canonicalize()?;
        let targets = channel.join("targets").canonicalize()?;
        if !metadata.is_dir() || !targets.is_dir() {
            return Err(invalid("channel must contain metadata/ and targets/ directories"));
        }
        Ok(Self { metadata, targets })
    }

    fn read(base: &Path, name: &str, maximum: u64) -> sigstore_tuf::Result<Option<Vec<u8>>> {
        if name.contains('\\') {
            return Err(sigstore_tuf::Error::Malformed(format!("unsafe repository path {name:?}")));
        }
        let relative = Path::new(name);
        if relative.is_absolute()
            || relative.components().any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(sigstore_tuf::Error::Malformed(format!("unsafe repository path {name:?}")));
        }
        let candidate = base.join(relative);
        let resolved = match candidate.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(sigstore_tuf::Error::Transport(format!(
                    "repository path resolution failed: {error}"
                )));
            }
        };
        if !resolved.starts_with(base) {
            return Err(sigstore_tuf::Error::Malformed("repository path escapes its root".into()));
        }
        let metadata = fs::metadata(&resolved)
            .map_err(|error| sigstore_tuf::Error::Transport(error.to_string()))?;
        if !metadata.is_file() || metadata.len() > maximum {
            return Err(sigstore_tuf::Error::Transport(format!(
                "repository file {name:?} exceeds {maximum} bytes or is not regular"
            )));
        }
        let bytes = fs::read(resolved)
            .map_err(|error| sigstore_tuf::Error::Transport(error.to_string()))?;
        if bytes.len() as u64 > maximum {
            return Err(sigstore_tuf::Error::Transport(format!(
                "repository file {name:?} grew beyond {maximum} bytes"
            )));
        }
        Ok(Some(bytes))
    }
}

impl sigstore_tuf::Repository for DirectoryRepository {
    fn fetch_metadata<'a>(&'a self, name: &'a str, max_length: u64) -> FetchFuture<'a> {
        let result = Self::read(&self.metadata, name, max_length);
        Box::pin(async move { result })
    }

    fn fetch_target<'a>(&'a self, path: &'a str, max_length: u64) -> FetchFuture<'a> {
        let result = Self::read(&self.targets, path, max_length);
        Box::pin(async move { result })
    }
}
