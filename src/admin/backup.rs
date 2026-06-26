use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use super::config::OwlConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMetadata {
    pub backup_id: String,
    pub backup_type: BackupType,
    pub timestamp: String,
    pub size_bytes: u64,
    pub lsn: u64,
    pub node_count: usize,
    pub checksum: String,
    pub source_data_dir: String,
    pub source_wal_dir: String,
    pub files: Vec<BackupFileMetadata>,
    pub deleted_files: Vec<DeletedFileMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupFileMetadata {
    pub area: BackupArea,
    pub relative_path: String,
    pub size_bytes: u64,
    pub checksum: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletedFileMetadata {
    pub area: BackupArea,
    pub relative_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum BackupArea {
    Data,
    Wal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackupType {
    Full,
    Incremental { base_backup_id: String },
}

pub struct BackupManager {
    config_path: PathBuf,
}

impl BackupManager {
    pub fn new(config_path: &PathBuf) -> Result<Self> {
        Ok(Self {
            config_path: config_path.clone(),
        })
    }

    pub fn create_backup(&self, destination: &Path, backup_type: &str) -> Result<()> {
        let backup_id = self.generate_backup_id();
        fs::create_dir_all(destination).with_context(|| {
            format!(
                "failed to create backup destination '{}'",
                destination.display()
            )
        })?;

        println!("Creating {} backup: {}", backup_type, backup_id);
        println!("Destination: {}", destination.display());
        println!();

        let backup_dir = destination.join(&backup_id);
        if backup_dir.exists() {
            bail!(
                "backup id collision: destination '{}' already exists",
                backup_dir.display()
            );
        }
        fs::create_dir_all(&backup_dir)
            .with_context(|| format!("failed to create backup dir '{}'", backup_dir.display()))?;

        let metadata = match backup_type {
            "full" => self.create_full_backup(&backup_dir, &backup_id)?,
            "incremental" => {
                self.create_incremental_backup(destination, &backup_dir, &backup_id)?
            }
            _ => bail!("Unknown backup type: {}", backup_type),
        };
        self.save_metadata(&backup_dir, &metadata)?;

        println!("\n✓ Backup created successfully");
        println!("  Backup ID: {}", backup_id);
        println!("  Location: {}", backup_dir.display());
        println!("  Files: {}", metadata.files.len());
        println!("  Size: {}", self.format_size(metadata.size_bytes));
        Ok(())
    }

    pub fn list_backups(&self, location: &Path) -> Result<()> {
        println!("Available Backups");
        println!("=================\n");

        let backups = self.scan_backups(location)?;
        if backups.is_empty() {
            println!("No backups found at {}", location.display());
            return Ok(());
        }

        println!(
            "{:<24} {:<14} {:<22} {:<12} {:<8}",
            "Backup ID", "Type", "Timestamp", "Size", "Files"
        );
        println!("{}", "-".repeat(90));
        for backup in &backups {
            let type_str = match &backup.backup_type {
                BackupType::Full => "Full".to_string(),
                BackupType::Incremental { base_backup_id } => format!("Incr ({})", base_backup_id),
            };
            println!(
                "{:<24} {:<14} {:<22} {:<12} {:<8}",
                backup.backup_id,
                type_str,
                backup.timestamp,
                self.format_size(backup.size_bytes),
                backup.files.len()
            );
        }
        println!("\nTotal backups: {}", backups.len());
        Ok(())
    }

    pub fn restore(&self, source: &Path, lsn: Option<u64>) -> Result<()> {
        self.restore_with_options(source, lsn, false)
    }

    pub fn restore_with_options(
        &self,
        source: &Path,
        lsn: Option<u64>,
        dry_run: bool,
    ) -> Result<()> {
        println!("Restoring from backup");
        println!("Source: {}", source.display());
        if dry_run {
            println!("Mode: dry-run (no files will be changed)");
        }

        let metadata = self.load_metadata(source)?;
        if let Some(target_lsn) = lsn {
            println!("Point-in-time recovery requested to LSN: {}", target_lsn);
            if target_lsn > metadata.lsn {
                bail!(
                    "requested LSN {} exceeds backup LSN {}",
                    target_lsn,
                    metadata.lsn
                );
            }
            if target_lsn < metadata.lsn {
                bail!("partial WAL replay to older LSN is not implemented yet");
            }
        }
        println!();

        println!("Step 1/4: Verifying backup integrity...");
        self.verify_chain(source)?;
        println!("  ✓ Backup is valid");

        println!("\nStep 2/4: Preparing target directories...");
        let config = self.load_or_default_config()?;
        fs::create_dir_all(&config.storage.data_dir).with_context(|| {
            format!(
                "failed to create data dir '{}'",
                config.storage.data_dir.display()
            )
        })?;
        fs::create_dir_all(&config.storage.wal_dir).with_context(|| {
            format!(
                "failed to create wal dir '{}'",
                config.storage.wal_dir.display()
            )
        })?;
        println!("  ✓ Target directories ready");

        if dry_run {
            let resolved = self.resolve_snapshot_map(source)?;
            let file_count = resolved.len();
            let total_bytes = resolved.values().map(|f| f.size_bytes).sum::<u64>();
            println!("\nStep 3/4: Restore plan");
            println!("  Files that would be restored: {}", file_count);
            println!("  Total logical size: {}", self.format_size(total_bytes));
            println!("\n✓ Dry-run complete");
            return Ok(());
        }

        println!("\nStep 3/4: Restoring backup chain...");
        self.restore_chain(source, &config, true)?;
        println!("  ✓ Restore data copied");

        println!("\nStep 4/4: Final verification...");
        self.verify_restored_target(source, &config)?;
        println!("  ✓ Restored files verified");

        println!("\n✓ Restore complete");
        Ok(())
    }

    pub fn verify(&self, path: &Path) -> Result<()> {
        println!("Verifying backup integrity");
        println!("Backup: {}", path.display());
        println!();
        self.verify_chain(path)?;
        println!("\n✓ Backup verification successful");
        println!("  Backup is ready for restore");
        Ok(())
    }

    fn create_full_backup(&self, backup_dir: &Path, backup_id: &str) -> Result<BackupMetadata> {
        let config = self.load_or_default_config()?;
        let data_dst = backup_dir.join("data");
        let wal_dst = backup_dir.join("wal");
        fs::create_dir_all(&data_dst)?;
        fs::create_dir_all(&wal_dst)?;

        let mut files = Vec::new();
        let mut total_size = 0u64;
        total_size += self.copy_area_files(
            &config.storage.data_dir,
            &data_dst,
            BackupArea::Data,
            &mut files,
        )?;
        total_size += self.copy_area_files(
            &config.storage.wal_dir,
            &wal_dst,
            BackupArea::Wal,
            &mut files,
        )?;

        let checksum = self.compute_metadata_checksum(&files, &[])?;
        Ok(BackupMetadata {
            backup_id: backup_id.to_string(),
            backup_type: BackupType::Full,
            timestamp: chrono::Local::now().to_rfc3339(),
            size_bytes: total_size,
            lsn: self.estimate_lsn(&config.storage.wal_dir)?,
            node_count: 1,
            checksum,
            source_data_dir: config.storage.data_dir.display().to_string(),
            source_wal_dir: config.storage.wal_dir.display().to_string(),
            files,
            deleted_files: Vec::new(),
        })
    }

    fn create_incremental_backup(
        &self,
        location: &Path,
        backup_dir: &Path,
        backup_id: &str,
    ) -> Result<BackupMetadata> {
        let base_backup_id = self.find_latest_backup(location)?;
        let base_dir = location.join(&base_backup_id);
        let base_map = self.resolve_snapshot_map(&base_dir)?;

        let config = self.load_or_default_config()?;
        let data_dst = backup_dir.join("data");
        let wal_dst = backup_dir.join("wal");
        fs::create_dir_all(&data_dst)?;
        fs::create_dir_all(&wal_dst)?;

        let mut files = Vec::new();
        let mut total_size = 0u64;
        total_size += self.copy_incremental_area(
            &config.storage.data_dir,
            &data_dst,
            BackupArea::Data,
            &base_map,
            &mut files,
        )?;
        total_size += self.copy_incremental_area(
            &config.storage.wal_dir,
            &wal_dst,
            BackupArea::Wal,
            &base_map,
            &mut files,
        )?;

        let mut deleted_files = Vec::new();
        for ((area, rel), _base_meta) in &base_map {
            if !self.source_file_exists(&config, area, rel) {
                deleted_files.push(DeletedFileMetadata {
                    area: area.clone(),
                    relative_path: rel.clone(),
                });
            }
        }

        let checksum = self.compute_metadata_checksum(&files, &deleted_files)?;
        Ok(BackupMetadata {
            backup_id: backup_id.to_string(),
            backup_type: BackupType::Incremental {
                base_backup_id: base_backup_id.clone(),
            },
            timestamp: chrono::Local::now().to_rfc3339(),
            size_bytes: total_size,
            lsn: self.estimate_lsn(&config.storage.wal_dir)?,
            node_count: 1,
            checksum,
            source_data_dir: config.storage.data_dir.display().to_string(),
            source_wal_dir: config.storage.wal_dir.display().to_string(),
            files,
            deleted_files,
        })
    }

    fn verify_internal(&self, path: &Path) -> Result<()> {
        let metadata = self.load_metadata(path)?;

        println!("Checking backup structure...");
        for file in &metadata.files {
            let on_disk = self.backup_file_path(path, file);
            if !on_disk.exists() {
                bail!("missing backup file '{}'", on_disk.display());
            }
        }
        println!("  ✓ Metadata and file paths present");

        println!("\nVerifying checksums...");
        for file in &metadata.files {
            let on_disk = self.backup_file_path(path, file);
            let checksum = self.checksum_file(&on_disk)?;
            if checksum != file.checksum {
                bail!(
                    "checksum mismatch for '{}': expected {}, got {}",
                    on_disk.display(),
                    file.checksum,
                    checksum
                );
            }
        }
        let aggregate = self.compute_metadata_checksum(&metadata.files, &metadata.deleted_files)?;
        if aggregate != metadata.checksum {
            bail!(
                "metadata checksum mismatch: expected {}, got {}",
                metadata.checksum,
                aggregate
            );
        }
        println!("  ✓ File checksums valid");

        if let BackupType::Incremental { base_backup_id } = &metadata.backup_type {
            let parent = path
                .parent()
                .ok_or_else(|| anyhow!("backup path has no parent"))?;
            let base = parent.join(base_backup_id);
            if !base.exists() {
                bail!("incremental base backup '{}' missing", base.display());
            }
            println!("  ✓ Incremental base backup present");
        }
        Ok(())
    }

    fn verify_chain(&self, path: &Path) -> Result<()> {
        let mut seen = HashSet::new();
        self.verify_chain_internal(path, &mut seen)
    }

    fn verify_chain_internal(&self, path: &Path, seen: &mut HashSet<String>) -> Result<()> {
        let metadata = self.load_metadata(path)?;
        if !seen.insert(metadata.backup_id.clone()) {
            bail!(
                "backup chain cycle detected at backup '{}'",
                metadata.backup_id
            );
        }
        self.verify_internal(path)?;
        if let BackupType::Incremental { base_backup_id } = metadata.backup_type {
            let parent = path
                .parent()
                .ok_or_else(|| anyhow!("backup path has no parent"))?;
            let base = parent.join(&base_backup_id);
            self.verify_chain_internal(&base, seen)?;
        }
        Ok(())
    }

    fn restore_chain(&self, source: &Path, config: &OwlConfig, clear_target: bool) -> Result<()> {
        let metadata = self.load_metadata(source)?;
        if let BackupType::Incremental { base_backup_id } = &metadata.backup_type {
            let parent = source
                .parent()
                .ok_or_else(|| anyhow!("backup path has no parent"))?;
            let base_path = parent.join(base_backup_id);
            self.restore_chain(&base_path, config, clear_target)?;
            self.apply_backup(source, config, false)?;
            return Ok(());
        }
        self.apply_backup(source, config, clear_target)
    }

    fn verify_restored_target(&self, source: &Path, config: &OwlConfig) -> Result<()> {
        let expected_map = self.resolve_snapshot_map(source)?;
        for ((area, rel), file_meta) in expected_map {
            let dst_root = match area {
                BackupArea::Data => &config.storage.data_dir,
                BackupArea::Wal => &config.storage.wal_dir,
            };
            let dst = dst_root.join(&rel);
            if !dst.exists() {
                bail!("restored file missing '{}'", dst.display());
            }
            let checksum = self.checksum_file(&dst)?;
            if checksum != file_meta.checksum {
                bail!(
                    "restored checksum mismatch for '{}': expected {}, got {}",
                    dst.display(),
                    file_meta.checksum,
                    checksum
                );
            }
        }
        Ok(())
    }

    fn apply_backup(&self, source: &Path, config: &OwlConfig, clear_target: bool) -> Result<()> {
        let metadata = self.load_metadata(source)?;

        if clear_target {
            self.clear_directory(&config.storage.data_dir)?;
            self.clear_directory(&config.storage.wal_dir)?;
        }

        for file in &metadata.files {
            let src = self.backup_file_path(source, file);
            let dst_root = match file.area {
                BackupArea::Data => &config.storage.data_dir,
                BackupArea::Wal => &config.storage.wal_dir,
            };
            let dst = dst_root.join(&file.relative_path);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&src, &dst).with_context(|| {
                format!(
                    "failed restoring '{}' to '{}'",
                    src.display(),
                    dst.display()
                )
            })?;
        }

        for deleted in &metadata.deleted_files {
            let dst_root = match deleted.area {
                BackupArea::Data => &config.storage.data_dir,
                BackupArea::Wal => &config.storage.wal_dir,
            };
            let dst = dst_root.join(&deleted.relative_path);
            if dst.exists() {
                fs::remove_file(&dst).with_context(|| {
                    format!(
                        "failed removing deleted file '{}' during restore",
                        dst.display()
                    )
                })?;
            }
        }

        Ok(())
    }

    fn scan_backups(&self, location: &Path) -> Result<Vec<BackupMetadata>> {
        if !location.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(location)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let meta_path = path.join("metadata.json");
            if !meta_path.exists() {
                continue;
            }
            let raw = fs::read_to_string(&meta_path)?;
            let metadata: BackupMetadata = serde_json::from_str(&raw)
                .with_context(|| format!("failed parsing '{}'", meta_path.display()))?;
            out.push(metadata);
        }
        out.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        Ok(out)
    }

    fn save_metadata(&self, backup_dir: &Path, metadata: &BackupMetadata) -> Result<()> {
        let metadata_path = backup_dir.join("metadata.json");
        let raw = serde_json::to_string_pretty(metadata)?;
        fs::write(&metadata_path, raw)
            .with_context(|| format!("failed writing metadata '{}'", metadata_path.display()))?;
        Ok(())
    }

    fn load_metadata(&self, backup_dir: &Path) -> Result<BackupMetadata> {
        let metadata_path = backup_dir.join("metadata.json");
        let raw = fs::read_to_string(&metadata_path)
            .with_context(|| format!("failed reading metadata '{}'", metadata_path.display()))?;
        let metadata: BackupMetadata = serde_json::from_str(&raw)
            .with_context(|| format!("failed parsing metadata '{}'", metadata_path.display()))?;
        Ok(metadata)
    }

    fn find_latest_backup(&self, location: &Path) -> Result<String> {
        let backups = self.scan_backups(location)?;
        backups
            .into_iter()
            .next()
            .map(|b| b.backup_id)
            .ok_or_else(|| anyhow!("no backup found in '{}'", location.display()))
    }

    fn copy_area_files(
        &self,
        source_root: &Path,
        backup_area_root: &Path,
        area: BackupArea,
        out_files: &mut Vec<BackupFileMetadata>,
    ) -> Result<u64> {
        let mut total = 0u64;
        if !source_root.exists() {
            return Ok(0);
        }
        for file in self.walk_files(source_root)? {
            let rel = file.strip_prefix(source_root).unwrap_or(&file);
            let rel_str = rel.to_string_lossy().to_string();
            let dst = backup_area_root.join(rel);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&file, &dst)?;
            let size = fs::metadata(&dst)?.len();
            let checksum = self.checksum_file(&dst)?;
            out_files.push(BackupFileMetadata {
                area: area.clone(),
                relative_path: rel_str,
                size_bytes: size,
                checksum,
            });
            total += size;
        }
        Ok(total)
    }

    fn copy_incremental_area(
        &self,
        source_root: &Path,
        backup_area_root: &Path,
        area: BackupArea,
        base_map: &HashMap<(BackupArea, String), BackupFileMetadata>,
        out_files: &mut Vec<BackupFileMetadata>,
    ) -> Result<u64> {
        let mut total = 0u64;
        if !source_root.exists() {
            return Ok(0);
        }
        for file in self.walk_files(source_root)? {
            let rel = file.strip_prefix(source_root).unwrap_or(&file);
            let rel_str = rel.to_string_lossy().to_string();
            let current_checksum = self.checksum_file(&file)?;
            let key = (area.clone(), rel_str.clone());
            let unchanged = base_map
                .get(&key)
                .map(|meta| meta.checksum == current_checksum)
                .unwrap_or(false);
            if unchanged {
                continue;
            }

            let dst = backup_area_root.join(rel);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&file, &dst)?;
            let size = fs::metadata(&dst)?.len();
            out_files.push(BackupFileMetadata {
                area: area.clone(),
                relative_path: rel_str,
                size_bytes: size,
                checksum: current_checksum,
            });
            total += size;
        }
        Ok(total)
    }

    fn walk_files(&self, root: &Path) -> Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        if !root.exists() {
            return Ok(out);
        }
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.is_file() {
                    out.push(path);
                }
            }
        }
        Ok(out)
    }

    fn clear_directory(&self, dir: &Path) -> Result<()> {
        if !dir.exists() {
            fs::create_dir_all(dir)?;
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                fs::remove_dir_all(&path)?;
            } else {
                fs::remove_file(&path)?;
            }
        }
        Ok(())
    }

    fn checksum_file(&self, path: &Path) -> Result<String> {
        let mut file = fs::File::open(path)?;
        let mut hasher = crc32fast::Hasher::new();
        let mut buf = [0u8; 8192];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(format!("{:08x}", hasher.finalize()))
    }

    fn compute_metadata_checksum(
        &self,
        files: &[BackupFileMetadata],
        deleted_files: &[DeletedFileMetadata],
    ) -> Result<String> {
        let mut sorted_files = files.to_vec();
        sorted_files.sort_by(|a, b| {
            (format!("{:?}", a.area), &a.relative_path)
                .cmp(&(format!("{:?}", b.area), &b.relative_path))
        });
        let mut sorted_deleted = deleted_files.to_vec();
        sorted_deleted.sort_by(|a, b| {
            (format!("{:?}", a.area), &a.relative_path)
                .cmp(&(format!("{:?}", b.area), &b.relative_path))
        });
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(serde_json::to_string(&sorted_files)?.as_bytes());
        hasher.update(serde_json::to_string(&sorted_deleted)?.as_bytes());
        Ok(format!("{:08x}", hasher.finalize()))
    }

    fn backup_file_path(&self, backup_root: &Path, file: &BackupFileMetadata) -> PathBuf {
        let area = match file.area {
            BackupArea::Data => "data",
            BackupArea::Wal => "wal",
        };
        backup_root.join(area).join(&file.relative_path)
    }

    fn source_file_exists(&self, config: &OwlConfig, area: &BackupArea, rel: &str) -> bool {
        let root = match area {
            BackupArea::Data => &config.storage.data_dir,
            BackupArea::Wal => &config.storage.wal_dir,
        };
        root.join(rel).exists()
    }

    fn estimate_lsn(&self, wal_dir: &Path) -> Result<u64> {
        if !wal_dir.exists() {
            return Ok(0);
        }
        let mut newest = 0u64;
        for file in self.walk_files(wal_dir)? {
            let meta = fs::metadata(&file)?;
            let modified = meta.modified().ok();
            let ts = modified
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            if ts > newest {
                newest = ts;
            }
        }
        Ok(newest)
    }

    fn resolve_snapshot_map(
        &self,
        backup_dir: &Path,
    ) -> Result<HashMap<(BackupArea, String), BackupFileMetadata>> {
        let mut visited = HashSet::new();
        self.resolve_snapshot_map_internal(backup_dir, &mut visited)
    }

    fn resolve_snapshot_map_internal(
        &self,
        backup_dir: &Path,
        visited: &mut HashSet<String>,
    ) -> Result<HashMap<(BackupArea, String), BackupFileMetadata>> {
        let metadata = self.load_metadata(backup_dir)?;
        if !visited.insert(metadata.backup_id.clone()) {
            bail!(
                "backup chain cycle detected while resolving snapshot at '{}'",
                metadata.backup_id
            );
        }
        let mut map = match &metadata.backup_type {
            BackupType::Full => HashMap::new(),
            BackupType::Incremental { base_backup_id } => {
                let parent = backup_dir
                    .parent()
                    .ok_or_else(|| anyhow!("backup path has no parent"))?;
                let base_dir = parent.join(base_backup_id);
                self.resolve_snapshot_map_internal(&base_dir, visited)?
            }
        };

        for deleted in &metadata.deleted_files {
            map.remove(&(deleted.area.clone(), deleted.relative_path.clone()));
        }
        for file in &metadata.files {
            map.insert(
                (file.area.clone(), file.relative_path.clone()),
                file.clone(),
            );
        }
        Ok(map)
    }

    fn load_or_default_config(&self) -> Result<OwlConfig> {
        if self.config_path.exists() {
            let raw = fs::read_to_string(&self.config_path).with_context(|| {
                format!(
                    "failed reading admin config '{}'",
                    self.config_path.display()
                )
            })?;
            let cfg: OwlConfig = toml::from_str(&raw).with_context(|| {
                format!(
                    "failed parsing admin config '{}'",
                    self.config_path.display()
                )
            })?;
            return Ok(cfg);
        }

        Ok(OwlConfig {
            cluster: super::config::ClusterConfig {
                node_id: 1,
                listen_address: "0.0.0.0:8080".to_string(),
                peers: vec![],
            },
            storage: super::config::StorageConfig {
                data_dir: PathBuf::from("./oql_data"),
                wal_dir: PathBuf::from("./oql_data"),
                max_arena_size_gb: 100,
                compression_enabled: true,
            },
            replication: super::config::ReplicationConfig {
                replication_factor: 1,
                election_timeout_ms: 2000,
                heartbeat_interval_ms: 500,
            },
            performance: super::config::PerformanceConfig {
                max_connections: 100,
                query_timeout_ms: 30000,
                cache_size_mb: 256,
            },
        })
    }

    fn generate_backup_id(&self) -> String {
        format!(
            "backup-{}",
            chrono::Local::now().format("%Y-%m-%d-%H%M%S-%6f")
        )
    }

    fn format_size(&self, bytes: u64) -> String {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;

        if bytes >= GB {
            format!("{:.2} GB", bytes as f64 / GB as f64)
        } else if bytes >= MB {
            format!("{:.2} MB", bytes as f64 / MB as f64)
        } else if bytes >= KB {
            format!("{:.2} KB", bytes as f64 / KB as f64)
        } else {
            format!("{} B", bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_test_config(path: &Path, data_dir: &Path, wal_dir: &Path) {
        let cfg = format!(
            r#"[cluster]
node_id = 1
listen_address = "127.0.0.1:8080"
peers = []

[storage]
data_dir = "{}"
wal_dir = "{}"
max_arena_size_gb = 1
compression_enabled = false

[replication]
replication_factor = 1
election_timeout_ms = 1000
heartbeat_interval_ms = 200

[performance]
max_connections = 10
query_timeout_ms = 1000
cache_size_mb = 64
"#,
            data_dir.display(),
            wal_dir.display()
        );
        fs::write(path, cfg).unwrap();
    }

    #[test]
    fn test_full_backup_restore_roundtrip() {
        let temp = TempDir::new().unwrap();
        let data_dir = temp.path().join("data");
        let wal_dir = temp.path().join("wal");
        let backup_dir = temp.path().join("backups");
        fs::create_dir_all(&data_dir).unwrap();
        fs::create_dir_all(&wal_dir).unwrap();
        fs::create_dir_all(&backup_dir).unwrap();

        fs::write(data_dir.join("main.owl"), b"before").unwrap();
        fs::write(wal_dir.join("main.owl.wal"), b"wal_before").unwrap();

        let cfg = temp.path().join("owl.toml");
        write_test_config(&cfg, &data_dir, &wal_dir);
        let manager = BackupManager::new(&cfg).unwrap();
        manager.create_backup(&backup_dir, "full").unwrap();

        // mutate after backup
        fs::write(data_dir.join("main.owl"), b"after").unwrap();
        fs::write(wal_dir.join("main.owl.wal"), b"wal_after").unwrap();

        let backup_entry = fs::read_dir(&backup_dir)
            .unwrap()
            .find_map(|e| {
                let p = e.unwrap().path();
                if p.is_dir() {
                    Some(p)
                } else {
                    None
                }
            })
            .unwrap();
        manager.restore(&backup_entry, None).unwrap();

        let data = fs::read(data_dir.join("main.owl")).unwrap();
        let wal = fs::read(wal_dir.join("main.owl.wal")).unwrap();
        assert_eq!(data, b"before");
        assert_eq!(wal, b"wal_before");
    }

    #[test]
    fn test_incremental_chain_restore_with_deletes() {
        let temp = TempDir::new().unwrap();
        let data_dir = temp.path().join("data");
        let wal_dir = temp.path().join("wal");
        let backup_dir = temp.path().join("backups");
        fs::create_dir_all(&data_dir).unwrap();
        fs::create_dir_all(&wal_dir).unwrap();
        fs::create_dir_all(&backup_dir).unwrap();

        fs::write(data_dir.join("a.owl"), b"a_v1").unwrap();
        fs::write(data_dir.join("b.owl"), b"b_v1").unwrap();
        fs::write(wal_dir.join("main.wal"), b"wal_v1").unwrap();

        let cfg = temp.path().join("owl.toml");
        write_test_config(&cfg, &data_dir, &wal_dir);
        let manager = BackupManager::new(&cfg).unwrap();
        manager.create_backup(&backup_dir, "full").unwrap();

        fs::write(data_dir.join("a.owl"), b"a_v2").unwrap();
        fs::remove_file(data_dir.join("b.owl")).unwrap();
        fs::write(data_dir.join("c.owl"), b"c_v1").unwrap();
        fs::write(wal_dir.join("main.wal"), b"wal_v2").unwrap();
        manager.create_backup(&backup_dir, "incremental").unwrap();

        fs::write(data_dir.join("a.owl"), b"a_local").unwrap();
        fs::write(data_dir.join("b.owl"), b"b_local").unwrap();
        fs::remove_file(data_dir.join("c.owl")).unwrap();
        fs::write(wal_dir.join("main.wal"), b"wal_local").unwrap();

        let latest = fs::read_dir(&backup_dir)
            .unwrap()
            .filter_map(|e| {
                let p = e.unwrap().path();
                if p.is_dir() {
                    Some(p)
                } else {
                    None
                }
            })
            .max()
            .unwrap();
        manager.restore(&latest, None).unwrap();

        assert_eq!(fs::read(data_dir.join("a.owl")).unwrap(), b"a_v2");
        assert!(!data_dir.join("b.owl").exists());
        assert_eq!(fs::read(data_dir.join("c.owl")).unwrap(), b"c_v1");
        assert_eq!(fs::read(wal_dir.join("main.wal")).unwrap(), b"wal_v2");
    }

    #[test]
    fn test_restore_dry_run_does_not_mutate_data() {
        let temp = TempDir::new().unwrap();
        let data_dir = temp.path().join("data");
        let wal_dir = temp.path().join("wal");
        let backup_dir = temp.path().join("backups");
        fs::create_dir_all(&data_dir).unwrap();
        fs::create_dir_all(&wal_dir).unwrap();
        fs::create_dir_all(&backup_dir).unwrap();

        fs::write(data_dir.join("main.owl"), b"before").unwrap();
        fs::write(wal_dir.join("main.owl.wal"), b"wal_before").unwrap();

        let cfg = temp.path().join("owl.toml");
        write_test_config(&cfg, &data_dir, &wal_dir);
        let manager = BackupManager::new(&cfg).unwrap();
        manager.create_backup(&backup_dir, "full").unwrap();

        fs::write(data_dir.join("main.owl"), b"after").unwrap();
        fs::write(wal_dir.join("main.owl.wal"), b"wal_after").unwrap();

        let backup_entry = fs::read_dir(&backup_dir)
            .unwrap()
            .find_map(|e| {
                let p = e.unwrap().path();
                if p.is_dir() {
                    Some(p)
                } else {
                    None
                }
            })
            .unwrap();

        manager
            .restore_with_options(&backup_entry, None, true)
            .unwrap();

        assert_eq!(fs::read(data_dir.join("main.owl")).unwrap(), b"after");
        assert_eq!(
            fs::read(wal_dir.join("main.owl.wal")).unwrap(),
            b"wal_after"
        );
    }
}
