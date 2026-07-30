use std::{
    collections::HashSet,
    error::Error,
    fmt, fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

use semantic_engine_core::{
    AnswerTarget, MAX_ALIASES_PER_TARGET, MAX_EXPRESSION_CHARS, MAX_IDENTIFIER_CHARS,
};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_DESCRIPTOR_BYTES: u64 = 256 * 1024;
const MAX_RESOURCE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_IMPORTED_TARGETS: usize = 50_000;
const SUPPORTED_FORMAT_VERSION: &str = "0.1.0";
const SUPPORTED_PACKAGE_SCHEMA: &str = "profile/context-package.schema.json";
const SUPPORTED_TARGET_SCHEMA: &str = "profile/title-resource.schema.json";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SourceMetadata {
    pub title: String,
    pub path: Option<String>,
    pub version: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImportedContext {
    pub name: String,
    pub locales: Vec<String>,
    pub spdx_license_expression: String,
    pub sources: Vec<SourceMetadata>,
    pub package_sha256: String,
    pub targets_sha256: String,
    pub id: String,
    pub version: Version,
    pub targets: Vec<AnswerTarget>,
}

#[derive(Debug)]
pub enum PackageError {
    Io(io::Error),
    Json(serde_json::Error),
    Invalid(&'static str),
    Integrity { expected: String, actual: String },
}

impl fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::Json(error) => write!(formatter, "invalid JSON: {error}"),
            Self::Invalid(reason) => write!(formatter, "invalid context package: {reason}"),
            Self::Integrity { expected, actual } => {
                write!(formatter, "resource hash mismatch: expected {expected}, got {actual}")
            }
        }
    }
}

impl Error for PackageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Invalid(_) | Self::Integrity { .. } => None,
        }
    }
}

impl From<io::Error> for PackageError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for PackageError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Deserialize)]
struct Descriptor {
    #[serde(rename = "$schema")]
    schema: String,
    name: String,
    id: String,
    version: String,
    licenses: Vec<License>,
    sources: Vec<SourceMetadata>,
    #[serde(rename = "semanticEngine")]
    semantic_engine: PackageMarker,
    resources: Vec<Resource>,
}

#[derive(Deserialize)]
struct License {
    name: Option<String>,
    path: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
struct PackageMarker {
    format_version: String,
    kind: String,
    locales: Vec<String>,
    spdx_license_expression: String,
}

#[derive(Deserialize)]
struct Resource {
    name: String,
    path: String,
    bytes: u64,
    hash: String,
    #[serde(rename = "semanticEngine")]
    semantic_engine: Option<ResourceMarker>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceMarker {
    role: String,
    schema: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TitleDocument {
    version: u32,
    titles: Vec<Title>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Title {
    id: String,
    #[serde(rename = "kind")]
    _kind: TitleKind,
    canonical: String,
    aliases: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum TitleKind {
    Movie,
    Game,
    Other,
}

pub fn import_package(descriptor_path: impl AsRef<Path>) -> Result<ImportedContext, PackageError> {
    let descriptor_path = descriptor_path.as_ref().canonicalize()?;
    let package_root = descriptor_path
        .parent()
        .ok_or(PackageError::Invalid("descriptor has no parent directory"))?
        .canonicalize()?;
    let descriptor_bytes = read_limited(&descriptor_path, MAX_DESCRIPTOR_BYTES)?;
    let descriptor: Descriptor = serde_json::from_slice(&descriptor_bytes)?;

    validate_descriptor(&descriptor)?;
    let version = Version::parse(&descriptor.version)
        .map_err(|_| PackageError::Invalid("version is not Semantic Versioning 2.0.0"))?;
    let resource = descriptor
        .resources
        .iter()
        .find(|resource| {
            resource.semantic_engine.as_ref().is_some_and(|marker| marker.role == "targets")
        })
        .ok_or(PackageError::Invalid("missing targets resource"))?;
    let resource_path = resolve_resource(&package_root, &resource.path)?;
    let resource_bytes = read_limited(&resource_path, MAX_RESOURCE_BYTES)?;

    if resource.bytes != resource_bytes.len() as u64 {
        return Err(PackageError::Invalid("declared resource byte size does not match"));
    }

    let actual_hash = format!("{:x}", Sha256::digest(&resource_bytes));
    let expected_hash = resource
        .hash
        .strip_prefix("sha256:")
        .ok_or(PackageError::Invalid("resource hash must use sha256"))?;
    if actual_hash != expected_hash {
        return Err(PackageError::Integrity {
            expected: expected_hash.to_owned(),
            actual: actual_hash,
        });
    }

    let document: TitleDocument = serde_json::from_slice(&resource_bytes)?;
    let targets = validate_titles(document)?;
    let mut package_hasher = Sha256::new();
    package_hasher.update(&descriptor_bytes);
    package_hasher.update([0]);
    package_hasher.update(&resource_bytes);
    let package_sha256 = format!("{:x}", package_hasher.finalize());

    Ok(ImportedContext {
        name: descriptor.name,
        id: descriptor.id,
        version,
        locales: descriptor.semantic_engine.locales,
        spdx_license_expression: descriptor.semantic_engine.spdx_license_expression,
        sources: descriptor.sources,
        package_sha256,
        targets_sha256: expected_hash.to_owned(),
        targets,
    })
}

fn validate_descriptor(descriptor: &Descriptor) -> Result<(), PackageError> {
    if descriptor.schema != SUPPORTED_PACKAGE_SCHEMA
        || descriptor.name.is_empty()
        || !descriptor.name.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || ".-_".contains(character)
        })
        || descriptor.id.is_empty()
    {
        return Err(PackageError::Invalid("unsupported package schema, name, or id"));
    }
    if descriptor.licenses.is_empty()
        || descriptor.licenses.iter().any(|license| {
            license.name.as_deref().unwrap_or("").is_empty()
                || license.path.as_deref().is_some_and(str::is_empty)
        })
    {
        return Err(PackageError::Invalid("a data license is required"));
    }
    if descriptor.sources.is_empty()
        || descriptor.sources.iter().any(|source| source.title.trim().is_empty())
    {
        return Err(PackageError::Invalid("at least one data source is required"));
    }
    if descriptor.semantic_engine.format_version != SUPPORTED_FORMAT_VERSION
        || descriptor.semantic_engine.kind != "recognition-context"
        || descriptor.semantic_engine.locales.is_empty()
        || descriptor.semantic_engine.locales.iter().collect::<HashSet<_>>().len()
            != descriptor.semantic_engine.locales.len()
        || descriptor.semantic_engine.locales.iter().any(|locale| !valid_locale(locale))
    {
        return Err(PackageError::Invalid("unsupported Semantic Engine package profile"));
    }
    let expression = spdx::Expression::parse(&descriptor.semantic_engine.spdx_license_expression)
        .map_err(|_| PackageError::Invalid("invalid SPDX license expression"))?;
    let declared_licenses = descriptor
        .licenses
        .iter()
        .map(|license| {
            spdx::Licensee::parse(license.name.as_deref().unwrap_or_default()).map_err(|_| {
                PackageError::Invalid("Data Package license is not a valid SPDX license")
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !expression.evaluate(|requirement| {
        declared_licenses.iter().any(|license| license.satisfies(requirement))
    }) {
        return Err(PackageError::Invalid("Data Package licenses conflict with SPDX expression"));
    }
    let target_resources = descriptor
        .resources
        .iter()
        .filter(|resource| {
            resource.semantic_engine.as_ref().is_some_and(|marker| marker.role == "targets")
        })
        .count();
    if target_resources != 1
        || descriptor.resources.iter().any(|resource| {
            resource.name.is_empty()
                || resource.semantic_engine.as_ref().is_some_and(|marker| {
                    marker.role.is_empty()
                        || marker.schema.is_empty()
                        || (marker.role == "targets" && marker.schema != SUPPORTED_TARGET_SCHEMA)
                })
        })
    {
        return Err(PackageError::Invalid("exactly one supported targets resource is required"));
    }
    Ok(())
}

fn valid_locale(locale: &str) -> bool {
    let mut parts = locale.split('-');
    let language = parts.next().unwrap_or_default();
    let region = parts.next();
    let no_more = parts.next().is_none();
    let valid_language = matches!(language.len(), 2 | 3)
        && language.chars().all(|character| character.is_ascii_lowercase());
    let valid_region = region.is_none_or(|value| {
        value.len() == 2 && value.chars().all(|character| character.is_ascii_uppercase())
    });
    valid_language && valid_region && no_more
}

fn validate_titles(document: TitleDocument) -> Result<Vec<AnswerTarget>, PackageError> {
    if document.version != 1
        || document.titles.is_empty()
        || document.titles.len() > MAX_IMPORTED_TARGETS
    {
        return Err(PackageError::Invalid("unsupported or empty title resource"));
    }

    let mut ids = HashSet::with_capacity(document.titles.len());
    let mut targets = Vec::with_capacity(document.titles.len());
    for title in document.titles {
        let valid = !title.id.is_empty()
            && title.id.chars().count() <= MAX_IDENTIFIER_CHARS
            && title.id.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || ".-_".contains(character)
            })
            && ids.insert(title.id.clone())
            && !title.canonical.is_empty()
            && title.canonical.chars().count() <= MAX_EXPRESSION_CHARS
            && title.aliases.len() <= MAX_ALIASES_PER_TARGET
            && title.aliases.iter().collect::<HashSet<_>>().len() == title.aliases.len()
            && title
                .aliases
                .iter()
                .all(|alias| !alias.is_empty() && alias.chars().count() <= MAX_EXPRESSION_CHARS);
        if !valid {
            return Err(PackageError::Invalid("invalid or duplicate target"));
        }
        targets.push(AnswerTarget {
            id: title.id,
            canonical: title.canonical,
            aliases: title.aliases,
        });
    }
    Ok(targets)
}

fn resolve_resource(package_root: &Path, relative: &str) -> Result<PathBuf, PackageError> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(PackageError::Invalid("resource path must stay inside the package"));
    }
    let resolved = package_root.join(relative).canonicalize()?;
    if !resolved.starts_with(package_root) {
        return Err(PackageError::Invalid("resource path escapes the package"));
    }
    Ok(resolved)
}

#[test]
fn contradictory_license_metadata_is_rejected() {
    let mut descriptor: Descriptor =
        serde_json::from_str(include_str!("../../../packages/starter-titles/datapackage.json"))
            .expect("starter descriptor must deserialize");
    descriptor.semantic_engine.spdx_license_expression = "MIT".to_owned();
    assert!(validate_descriptor(&descriptor).is_err());
}

#[test]
fn more_than_one_targets_resource_is_rejected() {
    let mut descriptor: Descriptor =
        serde_json::from_str(include_str!("../../../packages/starter-titles/datapackage.json"))
            .expect("starter descriptor must deserialize");
    let duplicate: Descriptor =
        serde_json::from_str(include_str!("../../../packages/starter-titles/datapackage.json"))
            .expect("starter descriptor must deserialize");
    descriptor.resources.push(duplicate.resources.into_iter().next().expect("resource"));
    assert!(validate_descriptor(&descriptor).is_err());
}

fn read_limited(path: &Path, maximum: u64) -> Result<Vec<u8>, PackageError> {
    let file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err(PackageError::Invalid("file is absent, not regular, or too large"));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(maximum + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(PackageError::Invalid("file grew beyond the size limit while reading"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_and_absolute_resource_paths_are_rejected() {
        let root = Path::new("package");
        assert!(resolve_resource(root, "../secret.json").is_err());
        assert!(resolve_resource(root, "C:/secret.json").is_err());
    }
}
