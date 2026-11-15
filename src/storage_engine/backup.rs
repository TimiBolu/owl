//! Backup and restore.
//!
//! A backup is a directory holding a copy of the database's files as they
//! stood at one commit, and a manifest listing them with their sizes and
//! checksums. It is taken while the database is in use: commits wait for
//! the copy, reads do not. Restoring checks every file against the
//! manifest, copies them to a new place, opens the result and has the
//! store audit itself before handing it back.

use super::RustStore;
use crate::error::{OQLError, OQLResult};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The manifest's file name inside a backup directory. It is written last,
/// so a directory without it is a backup that did not finish.
const MANIFEST: &str = "owl-backup.json";

/// One file of a backup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupFile {
    pub name: String,
    pub bytes: u64,
    pub crc32: u32,
}

/// What a backup holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupInfo {
    /// The file name of the database that was backed up.
    pub database: String,
    /// The last commit the backup contains.
    pub committed_lsn: u64,
    /// When it was taken, in seconds since the Unix epoch.
    pub created_unix_seconds: u64,
    pub files: Vec<BackupFile>,
}

impl BackupInfo {
    /// The size of the backup's files together.
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|file| file.bytes).sum()
    }
}

fn failed(what: &str, path: &Path, error: impl std::fmt::Display) -> OQLError {
    OQLError::storage_error(&format!("backup: cannot {what} {}: {error}", path.display()), None)
}

fn refused(reason: String) -> OQLError {
    OQLError::storage_error(&format!("backup: {reason}"), None)
}

/// The name of the database file, which every file of the database starts
/// with.
fn database_name(database: &Path) -> OQLResult<String> {
    database
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .ok_or_else(|| refused(format!("{} is not a database path", database.display())))
}

/// The directory a database path sits in.
fn directory_of(database: &Path) -> PathBuf {
    match database.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// The files that make up a database: the file itself and everything named
/// `<database>.<something>` beside it.
fn database_files(database: &Path) -> OQLResult<Vec<String>> {
    let name = database_name(database)?;
    let directory = directory_of(database);
    let prefix = format!("{name}.");
    let mut files = Vec::new();
    for entry in fs::read_dir(&directory).map_err(|error| failed("list", &directory, error))? {
        let entry = entry.map_err(|error| failed("list", &directory, error))?;
        let Some(file) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let is_file = entry
            .file_type()
            .map_err(|error| failed("inspect", &entry.path(), error))?
            .is_file();
        if is_file && (file == name || file.starts_with(&prefix)) {
            files.push(file);
        }
    }
    files.sort();
    Ok(files)
}

/// Copy one file, sync the copy, and return its size and checksum.
fn copy_file(from: &Path, to: &Path) -> OQLResult<(u64, u32)> {
    let mut source = fs::File::open(from).map_err(|error| failed("read", from, error))?;
    let mut target = fs::File::create(to).map_err(|error| failed("create", to, error))?;
    let mut checksum = crc32fast::Hasher::new();
    let mut bytes = 0u64;
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = source
            .read(&mut buffer)
            .map_err(|error| failed("read", from, error))?;
        if read == 0 {
            break;
        }
        checksum.update(&buffer[..read]);
        target
            .write_all(&buffer[..read])
            .map_err(|error| failed("write", to, error))?;
        bytes += read as u64;
    }
    target.sync_all().map_err(|error| failed("sync", to, error))?;
    Ok((bytes, checksum.finalize()))
}

fn checksum_of(path: &Path) -> OQLResult<(u64, u32)> {
    let mut source = fs::File::open(path).map_err(|error| failed("read", path, error))?;
    let mut checksum = crc32fast::Hasher::new();
    let mut bytes = 0u64;
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = source
            .read(&mut buffer)
            .map_err(|error| failed("read", path, error))?;
        if read == 0 {
            break;
        }
        checksum.update(&buffer[..read]);
        bytes += read as u64;
    }
    Ok((bytes, checksum.finalize()))
}

fn sync_directory(directory: &Path) -> OQLResult<()> {
    fs::File::open(directory)
        .and_then(|handle| handle.sync_all())
        .map_err(|error| failed("sync", directory, error))
}

impl RustStore {
    /// Copy the database, as of its last commit, into `destination`.
    ///
    /// `destination` must not exist or must be an empty directory. While
    /// the files are copied, commits wait and reads carry on. The backup is
    /// complete once this returns: its files and its manifest are on disk.
    /// A transaction that has not committed is not in it.
    pub fn backup(&self, destination: impl AsRef<Path>) -> OQLResult<BackupInfo> {
        let destination = destination.as_ref();
        match fs::read_dir(destination) {
            Ok(mut entries) => {
                if entries.next().is_some() {
                    return Err(refused(format!(
                        "{} is not empty; a backup goes in a directory of its own",
                        destination.display()
                    )));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(destination)
                    .map_err(|error| failed("create", destination, error))?;
            }
            Err(error) => return Err(failed("open", destination, error)),
        }

        let outcome = self.with_files_frozen(|database, committed_lsn| {
            let directory = directory_of(database);
            let mut files = Vec::new();
            for name in database_files(database)? {
                let (bytes, crc32) = copy_file(&directory.join(&name), &destination.join(&name))?;
                files.push(BackupFile { name, bytes, crc32 });
            }
            Ok(BackupInfo {
                database: database_name(database)?,
                committed_lsn,
                created_unix_seconds: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |since| since.as_secs()),
                files,
            })
        });
        let info = match outcome {
            Ok(info) => info,
            Err(error) => {
                // Leave nothing that could be mistaken for a backup.
                let _ = fs::remove_dir_all(destination);
                return Err(error);
            }
        };

        // The manifest goes last, under its final name only once whole.
        let manifest = destination.join(MANIFEST);
        let staged = destination.join(format!("{MANIFEST}.writing"));
        let encoded = serde_json::to_vec_pretty(&info)
            .map_err(|error| failed("encode the manifest of", destination, error))?;
        fs::File::create(&staged)
            .and_then(|mut file| file.write_all(&encoded).and_then(|_| file.sync_all()))
            .and_then(|_| fs::rename(&staged, &manifest))
            .map_err(|error| failed("write the manifest of", destination, error))?;
        sync_directory(destination)?;
        Ok(info)
    }

    /// Check a backup against its manifest: every file present, of the
    /// size and with the checksum recorded, and nothing extra. Returns what
    /// the backup holds.
    pub fn verify_backup(backup: impl AsRef<Path>) -> OQLResult<BackupInfo> {
        let backup = backup.as_ref();
        let manifest = backup.join(MANIFEST);
        let encoded = fs::read(&manifest).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                refused(format!(
                    "{} has no manifest: it is not a backup, or one that did not finish",
                    backup.display()
                ))
            } else {
                failed("read", &manifest, error)
            }
        })?;
        let info: BackupInfo = serde_json::from_slice(&encoded)
            .map_err(|error| failed("understand the manifest", &manifest, error))?;
        for file in &info.files {
            // A manifest names files in its own directory and nowhere else.
            if file.name.contains(['/', '\\']) || file.name == ".." {
                return Err(refused(format!("the manifest names '{}', which is not a file of the backup", file.name)));
            }
            let path = backup.join(&file.name);
            let (bytes, crc32) = checksum_of(&path)?;
            if bytes != file.bytes || crc32 != file.crc32 {
                return Err(refused(format!(
                    "{} does not match the manifest: it is damaged or was changed",
                    path.display()
                )));
            }
        }
        let listed = fs::read_dir(backup)
            .map_err(|error| failed("list", backup, error))?
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .filter(|name| name != MANIFEST && !info.files.iter().any(|file| &file.name == name))
            .collect::<Vec<_>>();
        if let Some(extra) = listed.first() {
            return Err(refused(format!(
                "{} holds '{extra}', which the manifest does not list",
                backup.display()
            )));
        }
        Ok(info)
    }

    /// Bring a backup back as a database at `database`, and open it.
    ///
    /// The backup is verified first and is not changed. `database` must not
    /// already be a database: restoring never overwrites one. The restored
    /// store audits itself and must be at the commit the backup recorded,
    /// or nothing is left behind and the reason is returned.
    pub fn restore(
        backup: impl AsRef<Path>,
        database: impl AsRef<Path>,
        max_size_bytes: usize,
    ) -> OQLResult<RustStore> {
        let backup = backup.as_ref();
        let database = database.as_ref();
        let info = Self::verify_backup(backup)?;
        let name = database_name(database)?;
        let directory = directory_of(database);
        fs::create_dir_all(&directory).map_err(|error| failed("create", &directory, error))?;
        if let Some(existing) = database_files(database)?.first() {
            return Err(refused(format!(
                "{} already exists; restore to a path that is free",
                directory.join(existing).display()
            )));
        }

        // Each file keeps its role under the new database's name.
        let mut restored = Vec::new();
        let outcome = (|| {
            for file in &info.files {
                let role = file.name.strip_prefix(&info.database).ok_or_else(|| {
                    refused(format!("'{}' is not a file of database '{}'", file.name, info.database))
                })?;
                let target = directory.join(format!("{name}{role}"));
                restored.push(target.clone());
                let (bytes, crc32) = copy_file(&backup.join(&file.name), &target)?;
                if bytes != file.bytes || crc32 != file.crc32 {
                    return Err(refused(format!("{} changed while it was being restored", file.name)));
                }
            }
            sync_directory(&directory)?;
            let store = RustStore::new(database, max_size_bytes)?;
            store.verify_integrity()?;
            let committed = store.committed_lsn();
            if committed != info.committed_lsn {
                return Err(refused(format!(
                    "the restored database is at commit {committed}, the backup recorded {}",
                    info.committed_lsn
                )));
            }
            Ok(store)
        })();
        if outcome.is_err() {
            for path in database_files(database).unwrap_or_default() {
                let _ = fs::remove_file(directory.join(path));
            }
        }
        outcome
    }
}
