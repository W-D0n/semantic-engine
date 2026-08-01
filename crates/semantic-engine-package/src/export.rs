use std::{
    ffi::OsStr,
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use semver::Version;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{
    CONTEXT_PACKAGE_PROFILE, ContextAttachment, ContextPackageDraft, DATA_PACKAGE_PROFILE,
    Descriptor, ImportedContext, License, MAX_METADATA_ATTACHMENT_BYTES,
    MAX_METADATA_ATTACHMENTS_TOTAL_BYTES, PackageError, PackageMarker, Resource, ResourceMarker,
    SUPPORTED_FORMAT_VERSION, SUPPORTED_PACKAGE_SCHEMA, SUPPORTED_TARGET_SCHEMA,
    TITLE_RESOURCE_PROFILE, Title, TitleDocument, import_package, is_remote_metadata_path,
    validate_descriptor, validate_metadata_attachment_path,
};

pub fn export_package(
    draft: &ContextPackageDraft,
    new_version: &str,
    descriptor_path: impl AsRef<Path>,
) -> Result<ImportedContext, PackageError> {
    let base_version = Version::parse(&draft.base_version).map_err(|_| {
        PackageError::Invalid("draft base version is not Semantic Versioning 2.0.0")
    })?;
    let new_version = Version::parse(new_version)
        .map_err(|_| PackageError::Invalid("export version is not Semantic Versioning 2.0.0"))?;
    if new_version <= base_version {
        return Err(PackageError::Invalid("export version must be greater than its base version"));
    }
    if draft.target_kinds.len() != draft.targets.len()
        || draft.targets.iter().any(|target| !draft.target_kinds.contains_key(&target.id))
    {
        return Err(PackageError::Invalid("every exported target must retain its configured kind"));
    }
    let missing_attachment = draft
        .licenses
        .iter()
        .filter_map(|license| license.path.as_deref())
        .chain(draft.sources.iter().filter_map(|source| source.path.as_deref()))
        .any(|path| !is_remote_metadata_path(path) && !draft.attachments.contains_key(path));
    if missing_attachment {
        return Err(PackageError::Invalid(
            "every local license or source path must have an imported attachment",
        ));
    }

    let descriptor_path = descriptor_path.as_ref();
    if descriptor_path.file_name() != Some(OsStr::new("datapackage.json")) {
        return Err(PackageError::Invalid("export descriptor must be named datapackage.json"));
    }
    let destination = descriptor_path
        .parent()
        .ok_or(PackageError::Invalid("export descriptor has no parent directory"))?;
    if destination.exists() {
        return Err(PackageError::Invalid("export destination already exists"));
    }
    let parent = destination
        .parent()
        .ok_or(PackageError::Invalid("export destination has no parent directory"))?;
    fs::create_dir_all(parent)?;
    let temporary = create_temporary_directory(parent)?;

    let result = write_and_validate(draft, &new_version, &temporary).and_then(|imported| {
        publish_directory_no_replace(&temporary, destination)?;
        Ok(imported)
    });
    if result.is_err() && temporary.exists() {
        let _ = fs::remove_dir_all(&temporary);
    }
    result
}

fn write_and_validate(
    draft: &ContextPackageDraft,
    new_version: &Version,
    package_root: &Path,
) -> Result<ImportedContext, PackageError> {
    let titles = draft
        .targets
        .iter()
        .map(|target| Title {
            id: target.id.clone(),
            kind: draft.target_kinds[&target.id],
            canonical: target.canonical.clone(),
            aliases: target.aliases.clone(),
        })
        .collect();
    let title_bytes = pretty_json(&TitleDocument { version: 1, titles })?;
    let targets_sha256 = format!("{:x}", Sha256::digest(&title_bytes));

    let descriptor = Descriptor {
        schema: SUPPORTED_PACKAGE_SCHEMA.to_owned(),
        name: draft.name.clone(),
        id: draft.id.clone(),
        version: new_version.to_string(),
        licenses: draft
            .licenses
            .iter()
            .map(|license| License {
                name: Some(license.name.clone()),
                path: license.path.clone(),
                metadata: license.metadata.clone(),
            })
            .collect(),
        sources: draft.sources.clone(),
        semantic_engine: PackageMarker {
            format_version: SUPPORTED_FORMAT_VERSION.to_owned(),
            kind: "recognition-context".to_owned(),
            locales: draft.locales.clone(),
            spdx_license_expression: draft.spdx_license_expression.clone(),
        },
        resources: vec![Resource {
            name: "titles".to_owned(),
            path: "data/titles.json".to_owned(),
            bytes: title_bytes.len() as u64,
            hash: format!("sha256:{targets_sha256}"),
            semantic_engine: Some(ResourceMarker {
                role: "targets".to_owned(),
                schema: SUPPORTED_TARGET_SCHEMA.to_owned(),
            }),
            metadata: draft.targets_resource_metadata.clone(),
        }],
        metadata: draft.metadata.clone(),
    };
    validate_descriptor(&descriptor)?;
    let descriptor_bytes = pretty_json(&descriptor)?;

    fs::create_dir_all(package_root.join("data"))?;
    fs::create_dir_all(package_root.join("profile/vendor"))?;
    fs::write(package_root.join("data/titles.json"), title_bytes)?;
    fs::write(package_root.join("datapackage.json"), descriptor_bytes)?;
    fs::write(package_root.join("profile/context-package.schema.json"), offline_context_profile())?;
    fs::write(package_root.join("profile/title-resource.schema.json"), TITLE_RESOURCE_PROFILE)?;
    fs::write(
        package_root.join("profile/vendor/datapackage-v2.schema.json"),
        DATA_PACKAGE_PROFILE,
    )?;
    write_attachments(package_root, &draft.attachments)?;
    if !package_root.join("README.md").exists() {
        fs::write(package_root.join("README.md"), package_readme(draft, new_version))?;
    }
    if !package_root.join("LICENSE.md").exists() {
        fs::write(package_root.join("LICENSE.md"), license_notice(draft))?;
    }
    write_checksums(package_root)?;
    import_package(package_root.join("datapackage.json"))
}

fn offline_context_profile() -> String {
    CONTEXT_PACKAGE_PROFILE
        .replace(
            "\"$id\": \"urn:semantic-engine:profile:context-package:0.1.0\"",
            "\"$id\": \"context-package.schema.json\"",
        )
        .replace(
            "https://datapackage.org/profiles/2.0/datapackage.json",
            "vendor/datapackage-v2.schema.json",
        )
}

fn package_readme(draft: &ContextPackageDraft, version: &Version) -> String {
    format!(
        "# Exported recognition context\n\n- Package: `{}`\n- ID: `{}`\n- Version: `{version}`\n- Data license: `{}`\n- Targets: {}\n\nGenerated and validated locally by Semantic Engine. `datapackage.json` is the machine-readable source of truth.\n",
        markdown_inline(&draft.name),
        markdown_inline(&draft.id),
        markdown_inline(&draft.spdx_license_expression),
        draft.targets.len(),
    )
}

fn license_notice(draft: &ContextPackageDraft) -> String {
    let mut notice = format!(
        "# Data license\n\nSPDX expression: `{}`\n\nDeclared licenses:\n",
        markdown_inline(&draft.spdx_license_expression)
    );
    for license in &draft.licenses {
        notice.push_str(&format!("\n- `{}`", markdown_inline(&license.name)));
        if let Some(path) = license.path.as_deref().filter(|path| is_remote_metadata_path(path)) {
            notice.push_str(&format!(" — <{}>", markdown_inline(path)));
        }
    }
    notice.push_str("\n\nThis notice preserves the package metadata. Consult the linked license text or the SPDX identifier before redistributing modified data.\n");
    notice
}

fn markdown_inline(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('`', "&#96;")
}

fn write_attachments(
    package_root: &Path,
    attachments: &std::collections::BTreeMap<String, ContextAttachment>,
) -> Result<(), PackageError> {
    let mut portable_paths = std::collections::HashSet::new();
    let mut total_bytes = 0usize;
    for (relative, attachment) in attachments {
        validate_metadata_attachment_path(relative)?;
        if !portable_paths.insert(relative.to_ascii_lowercase()) {
            return Err(PackageError::Invalid(
                "metadata attachment paths collide on a case-insensitive file system",
            ));
        }
        let content = BASE64
            .decode(&attachment.content_base64)
            .map_err(|_| PackageError::Invalid("metadata attachment is not valid base64"))?;
        if content.len() as u64 > MAX_METADATA_ATTACHMENT_BYTES {
            return Err(PackageError::Invalid("metadata attachment exceeds its size limit"));
        }
        total_bytes = total_bytes.saturating_add(content.len());
        if total_bytes > MAX_METADATA_ATTACHMENTS_TOTAL_BYTES {
            return Err(PackageError::Invalid("local metadata attachments exceed the total limit"));
        }
        let relative_path = Path::new(relative);
        let destination = package_root.join(relative_path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(destination, content)?;
    }
    Ok(())
}

fn write_checksums(package_root: &Path) -> Result<(), PackageError> {
    let mut files = Vec::new();
    collect_package_files(package_root, package_root, &mut files)?;
    files.sort();
    let mut manifest = String::new();
    for path in files {
        let relative = path
            .strip_prefix(package_root)
            .map_err(|_| PackageError::Invalid("checksum path escaped the package root"))?
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let bytes = fs::read(path)?;
        manifest.push_str(&format!("{:x}  {relative}\n", Sha256::digest(bytes)));
    }
    fs::write(package_root.join("SHA256SUMS.txt"), manifest)?;
    Ok(())
}

fn collect_package_files(
    package_root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), PackageError> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path == package_root.join("SHA256SUMS.txt") {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_package_files(package_root, &path, files)?;
        } else if file_type.is_file() {
            files.push(path);
        } else {
            return Err(PackageError::Invalid("exported package contains a non-regular file"));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn publish_directory_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::{iter, os::windows::ffi::OsStrExt};

    let source = source.as_os_str().encode_wide().chain(iter::once(0)).collect::<Vec<_>>();
    let destination =
        destination.as_os_str().encode_wide().chain(iter::once(0)).collect::<Vec<_>>();
    let result = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            0,
        )
    };
    if result != 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

#[cfg(target_os = "linux")]
fn publish_directory_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let source = CString::new(source.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(ErrorKind::InvalidInput, "export source contains a null byte")
    })?;
    let destination = CString::new(destination.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(ErrorKind::InvalidInput, "export destination contains a null byte")
    })?;
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn publish_directory_no_replace(_source: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::Error::new(
        ErrorKind::Unsupported,
        "immutable context export is not yet supported on this operating system",
    ))
}

fn pretty_json<T: Serialize>(value: &T) -> Result<Vec<u8>, PackageError> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn create_temporary_directory(parent: &Path) -> Result<PathBuf, PackageError> {
    for sequence in 0..100 {
        let candidate =
            parent.join(format!(".semantic-engine-export-{}-{sequence}", std::process::id()));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(PackageError::Io(error)),
        }
    }
    Err(PackageError::Io(io::Error::new(
        ErrorKind::AlreadyExists,
        "could not allocate a temporary export directory",
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_clobber_publication_refuses_an_existing_empty_directory() {
        let workspace = tempfile::tempdir().expect("temporary workspace must be available");
        let source = workspace.path().join("source");
        let destination = workspace.path().join("destination");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("payload"), "new").unwrap();
        fs::create_dir(&destination).unwrap();

        publish_directory_no_replace(&source, &destination)
            .expect_err("publication must not replace a destination created by a racing writer");
        assert!(source.join("payload").is_file());
        assert!(destination.read_dir().unwrap().next().is_none());
    }
}
