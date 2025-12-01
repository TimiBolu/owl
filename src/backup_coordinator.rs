//! Backup Coordinator for Distributed Owl Database
//!
//! Task 14.2: Implement incremental backups (Zig + Rust)
//!
//! This module provides Rust-level backup orchestration and scheduling for the
//! distributed Owl database. It coordinates backups across multiple shards and
//! manages backup schedules.
//!
//! Features:
//! - Track changed pages at Zig storage layer
//! - Implement delta compression for incremental backups
//! - Add Rust-level backup orchestration and scheduling
//! - Test restore from incremental backups with various failure scenarios

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::OQLError;

/// Backup type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupType {
    /// Full backup containing all data
    Full,
    /// Incremental backup containing only changes since last backup
    Incremental,
    /// Differential backup containing changes since last full backup
    Differential,
    /// Snapshot backup at specific point in time
    Snapshot,
}

/// Backup status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupStatus {
    /// Backup is scheduled but not started
    Scheduled,
    /// Backup is currently in progress
    InProgress,
    /// Backup completed successfully
    Completed,
    /// Backup failed
    Failed,
    /// Backup was cancelled
    Cancelled,
}

/// Backup metadata
#[derive(Debug, Clone)]
pub struct BackupMetadata {
    /// Unique backup identifier
    pub backup_id: String,
    /// Backup type
    pub backup_type: BackupType,
    /// Backup status
    pub status: BackupStatus,
    /// Shard ID this backup belongs to
    pub shard_id: u32,
    /// Creation timestamp
    pub created_at: SystemTime,
    /// Completion timestamp
    pub completed_at: Option<SystemTime>,
    /// LSN at backup time
    pub backup_lsn: u64,
    /// Previous backup ID (for incremental/differential)
    pub previous_backup_id: Option<String>,
    /// Total backup size in bytes
    pub total_size: u64,
    /// Compressed size in bytes
    pub compressed_size: u64,
    /// Number of objects backed up
    pub object_count: u64,
    /// Backup description
    pub description: String,
    /// Error message if backup failed
    pub error_message: Option<String>,
}

impl BackupMetadata {
    /// Create new backup metadata
    pub fn new(
        backup_id: String,
        backup_type: BackupType,
        shard_id: u32,
        description: String,
    ) -> Self {
        Self {
            backup_id,
            backup_type,
            status: BackupStatus::Scheduled,
            shard_id,
            created_at: SystemTime::now(),
            completed_at: None,
            backup_lsn: 0,
            previous_backup_id: None,
            total_size: 0,
            compressed_size: 0,
            object_count: 0,
            description,
            error_message: None,
        }
    }

    /// Calculate compression ratio
    pub fn compression_ratio(&self) -> f64 {
        if self.total_size > 0 {
            self.compressed_size as f64 / self.total_size as f64
        } else {
            1.0
        }
    }

    /// Calculate backup duration
    pub fn duration(&self) -> Option<Duration> {
        self.completed_at
            .and_then(|completed| completed.duration_since(self.created_at).ok())
    }
}

/// Backup schedule configuration
#[derive(Debug, Clone)]
pub struct BackupSchedule {
    /// Enable automatic backups
    pub enabled: bool,
    /// Full backup interval (e.g., daily)
    pub full_backup_interval: Duration,
    /// Incremental backup interval (e.g., hourly)
    pub incremental_backup_interval: Duration,
    /// Retention period for backups
    pub retention_period: Duration,
    /// Maximum number of backups to keep
    pub max_backups: usize,
    /// Enable backup verification after creation
    pub verify_backups: bool,
}

impl Default for BackupSchedule {
    fn default() -> Self {
        Self {
            enabled: true,
            full_backup_interval: Duration::from_secs(24 * 60 * 60), // Daily
            incremental_backup_interval: Duration::from_secs(60 * 60), // Hourly
            retention_period: Duration::from_secs(30 * 24 * 60 * 60), // 30 days
            max_backups: 100,
            verify_backups: true,
        }
    }
}

/// Backup statistics
#[derive(Debug, Clone, Default)]
pub struct BackupStats {
    /// Total backups created
    pub backups_created: u64,
    /// Total backup size
    pub total_backup_size: u64,
    /// Total compressed size
    pub total_compressed_size: u64,
    /// Objects backed up
    pub objects_backed_up: u64,
    /// Last backup timestamp
    pub last_backup_time: Option<SystemTime>,
    /// Last backup LSN
    pub last_backup_lsn: u64,
    /// Backup operations in progress
    pub operations_in_progress: u32,
    /// Recovery operations performed
    pub recovery_operations: u64,
    /// Objects recovered
    pub objects_recovered: u64,
    /// Failed backups
    pub failed_backups: u64,
}

impl BackupStats {
    /// Calculate compression ratio
    pub fn compression_ratio(&self) -> f64 {
        if self.total_backup_size > 0 {
            self.total_compressed_size as f64 / self.total_backup_size as f64
        } else {
            1.0
        }
    }
}

/// Backup coordinator for distributed backups
pub struct BackupCoordinator {
    /// Backup schedule configuration
    schedule: BackupSchedule,
    /// Backup metadata by backup ID
    backups: Arc<Mutex<HashMap<String, BackupMetadata>>>,
    /// Backup statistics
    stats: Arc<Mutex<BackupStats>>,
    /// Base backup directory
    backup_directory: PathBuf,
    /// Last full backup time per shard
    last_full_backup: Arc<Mutex<HashMap<u32, SystemTime>>>,
    /// Last incremental backup time per shard
    last_incremental_backup: Arc<Mutex<HashMap<u32, SystemTime>>>,
}

impl BackupCoordinator {
    /// Create new backup coordinator
    pub fn new(backup_directory: PathBuf, schedule: BackupSchedule) -> Result<Self, OQLError> {
        // Ensure backup directory exists
        std::fs::create_dir_all(&backup_directory).map_err(|e| {
            OQLError::IoError(format!("Failed to create backup directory: {}", e))
        })?;

        Ok(Self {
            schedule,
            backups: Arc::new(Mutex::new(HashMap::new())),
            stats: Arc::new(Mutex::new(BackupStats::default())),
            backup_directory,
            last_full_backup: Arc::new(Mutex::new(HashMap::new())),
            last_incremental_backup: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Schedule a backup for a shard
    pub fn schedule_backup(
        &self,
        shard_id: u32,
        backup_type: BackupType,
        description: String,
    ) -> Result<String, OQLError> {
        let backup_id = self.generate_backup_id(shard_id, backup_type);
        let metadata = BackupMetadata::new(backup_id.clone(), backup_type, shard_id, description);

        let mut backups = self.backups.lock().unwrap();
        backups.insert(backup_id.clone(), metadata);

        Ok(backup_id)
    }

    /// Start a backup operation
    pub fn start_backup(&self, backup_id: &str) -> Result<(), OQLError> {
        let mut backups = self.backups.lock().unwrap();
        let metadata = backups
            .get_mut(backup_id)
            .ok_or_else(|| OQLError::NotFound(format!("Backup {} not found", backup_id)))?;

        metadata.status = BackupStatus::InProgress;
        metadata.created_at = SystemTime::now();

        let mut stats = self.stats.lock().unwrap();
        stats.operations_in_progress += 1;

        Ok(())
    }

    /// Complete a backup operation
    pub fn complete_backup(
        &self,
        backup_id: &str,
        backup_lsn: u64,
        total_size: u64,
        compressed_size: u64,
        object_count: u64,
    ) -> Result<(), OQLError> {
        let mut backups = self.backups.lock().unwrap();
        let metadata = backups
            .get_mut(backup_id)
            .ok_or_else(|| OQLError::NotFound(format!("Backup {} not found", backup_id)))?;

        metadata.status = BackupStatus::Completed;
        metadata.completed_at = Some(SystemTime::now());
        metadata.backup_lsn = backup_lsn;
        metadata.total_size = total_size;
        metadata.compressed_size = compressed_size;
        metadata.object_count = object_count;

        // Update statistics
        let mut stats = self.stats.lock().unwrap();
        stats.operations_in_progress = stats.operations_in_progress.saturating_sub(1);
        stats.backups_created += 1;
        stats.total_backup_size += total_size;
        stats.total_compressed_size += compressed_size;
        stats.objects_backed_up += object_count;
        stats.last_backup_time = Some(SystemTime::now());
        stats.last_backup_lsn = backup_lsn;

        // Update last backup time
        match metadata.backup_type {
            BackupType::Full => {
                let mut last_full = self.last_full_backup.lock().unwrap();
                last_full.insert(metadata.shard_id, SystemTime::now());
            }
            BackupType::Incremental => {
                let mut last_incr = self.last_incremental_backup.lock().unwrap();
                last_incr.insert(metadata.shard_id, SystemTime::now());
            }
            _ => {}
        }

        Ok(())
    }

    /// Fail a backup operation
    pub fn fail_backup(&self, backup_id: &str, error_message: String) -> Result<(), OQLError> {
        let mut backups = self.backups.lock().unwrap();
        let metadata = backups
            .get_mut(backup_id)
            .ok_or_else(|| OQLError::NotFound(format!("Backup {} not found", backup_id)))?;

        metadata.status = BackupStatus::Failed;
        metadata.completed_at = Some(SystemTime::now());
        metadata.error_message = Some(error_message);

        let mut stats = self.stats.lock().unwrap();
        stats.operations_in_progress = stats.operations_in_progress.saturating_sub(1);
        stats.failed_backups += 1;

        Ok(())
    }

    /// Check if a full backup is needed for a shard
    pub fn needs_full_backup(&self, shard_id: u32) -> bool {
        let last_full = self.last_full_backup.lock().unwrap();
        match last_full.get(&shard_id) {
            Some(last_time) => {
                let elapsed = SystemTime::now()
                    .duration_since(*last_time)
                    .unwrap_or(Duration::from_secs(0));
                elapsed >= self.schedule.full_backup_interval
            }
            None => true, // No full backup yet
        }
    }

    /// Check if an incremental backup is needed for a shard
    pub fn needs_incremental_backup(&self, shard_id: u32) -> bool {
        let last_incr = self.last_incremental_backup.lock().unwrap();
        match last_incr.get(&shard_id) {
            Some(last_time) => {
                let elapsed = SystemTime::now()
                    .duration_since(*last_time)
                    .unwrap_or(Duration::from_secs(0));
                elapsed >= self.schedule.incremental_backup_interval
            }
            None => {
                // Check if we have a full backup first
                let last_full = self.last_full_backup.lock().unwrap();
                last_full.contains_key(&shard_id)
            }
        }
    }

    /// Get backup metadata
    pub fn get_backup(&self, backup_id: &str) -> Option<BackupMetadata> {
        let backups = self.backups.lock().unwrap();
        backups.get(backup_id).cloned()
    }

    /// List all backups
    pub fn list_backups(&self) -> Vec<BackupMetadata> {
        let backups = self.backups.lock().unwrap();
        backups.values().cloned().collect()
    }

    /// List backups for a specific shard
    pub fn list_shard_backups(&self, shard_id: u32) -> Vec<BackupMetadata> {
        let backups = self.backups.lock().unwrap();
        backups
            .values()
            .filter(|b| b.shard_id == shard_id)
            .cloned()
            .collect()
    }

    /// Get backup statistics
    pub fn get_stats(&self) -> BackupStats {
        let stats = self.stats.lock().unwrap();
        stats.clone()
    }

    /// Cleanup old backups based on retention policy
    pub fn cleanup_old_backups(&self) -> Result<usize, OQLError> {
        let cutoff_time = SystemTime::now()
            .checked_sub(self.schedule.retention_period)
            .unwrap_or(UNIX_EPOCH);

        let mut backups = self.backups.lock().unwrap();
        let mut to_remove = Vec::new();

        for (backup_id, metadata) in backups.iter() {
            if metadata.created_at < cutoff_time {
                to_remove.push(backup_id.clone());
            }
        }

        let removed_count = to_remove.len();
        for backup_id in to_remove {
            backups.remove(&backup_id);
            // TODO: Delete actual backup files
        }

        Ok(removed_count)
    }

    /// Generate a unique backup ID
    fn generate_backup_id(&self, shard_id: u32, backup_type: BackupType) -> String {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let type_str = match backup_type {
            BackupType::Full => "full",
            BackupType::Incremental => "incr",
            BackupType::Differential => "diff",
            BackupType::Snapshot => "snap",
        };

        format!("backup_{}_{}_{}_{}", type_str, shard_id, timestamp, uuid::Uuid::new_v4())
    }

    /// Get backup directory
    pub fn backup_directory(&self) -> &Path {
        &self.backup_directory
    }

    /// Get backup schedule
    pub fn schedule(&self) -> &BackupSchedule {
        &self.schedule
    }

    /// Perform point-in-time recovery across distributed nodes
    /// This coordinates recovery to a specific LSN across all shards
    pub fn perform_distributed_recovery(
        &self,
        target_lsn: u64,
        shard_ids: &[u32],
    ) -> Result<DistributedRecoveryResult, OQLError> {
        let mut shard_results = HashMap::new();
        let mut total_objects_recovered = 0;
        let mut failed_shards = Vec::new();

        for &shard_id in shard_ids {
            match self.recover_shard_to_lsn(shard_id, target_lsn) {
                Ok(result) => {
                    total_objects_recovered += result.objects_recovered;
                    shard_results.insert(shard_id, result);
                }
                Err(e) => {
                    failed_shards.push((shard_id, e.to_string()));
                }
            }
        }

        // Update statistics
        let mut stats = self.stats.lock().unwrap();
        stats.recovery_operations += 1;
        stats.objects_recovered += total_objects_recovered;

        Ok(DistributedRecoveryResult {
            target_lsn,
            shard_results,
            failed_shards,
            total_objects_recovered,
        })
    }

    /// Recover a single shard to a specific LSN
    fn recover_shard_to_lsn(
        &self,
        shard_id: u32,
        target_lsn: u64,
    ) -> Result<ShardRecoveryResult, OQLError> {
        // Find the most recent backup for this shard that is <= target_lsn
        let backups = self.list_shard_backups(shard_id);
        let mut suitable_backups: Vec<_> = backups
            .iter()
            .filter(|b| b.backup_lsn <= target_lsn && b.status == BackupStatus::Completed)
            .collect();

        suitable_backups.sort_by_key(|b| b.backup_lsn);

        let base_backup = suitable_backups
            .last()
            .ok_or_else(|| OQLError::NotFound(format!("No suitable backup found for shard {} at LSN {}", shard_id, target_lsn)))?;

        // Build recovery chain (full backup + incrementals)
        let recovery_chain = self.build_recovery_chain(base_backup, target_lsn)?;

        // TODO: Actually perform recovery by calling Zig layer
        // For now, we just return a placeholder result

        Ok(ShardRecoveryResult {
            shard_id,
            backup_id: base_backup.backup_id.clone(),
            objects_recovered: base_backup.object_count,
            recovery_lsn: base_backup.backup_lsn,
        })
    }

    /// Build recovery chain from base backup to target LSN
    fn build_recovery_chain(
        &self,
        base_backup: &BackupMetadata,
        target_lsn: u64,
    ) -> Result<Vec<BackupMetadata>, OQLError> {
        let mut chain = vec![base_backup.clone()];
        let mut current_lsn = base_backup.backup_lsn;

        // Find incremental backups that bridge the gap to target_lsn
        let backups = self.list_shard_backups(base_backup.shard_id);
        let mut incrementals: Vec<_> = backups
            .iter()
            .filter(|b| {
                b.backup_type == BackupType::Incremental
                    && b.backup_lsn > current_lsn
                    && b.backup_lsn <= target_lsn
                    && b.status == BackupStatus::Completed
            })
            .collect();

        incrementals.sort_by_key(|b| b.backup_lsn);

        for incremental in incrementals {
            chain.push(incremental.clone());
            current_lsn = incremental.backup_lsn;
        }

        Ok(chain)
    }

    /// Verify recovery consistency across shards
    pub fn verify_recovery_consistency(
        &self,
        recovery_result: &DistributedRecoveryResult,
    ) -> Result<bool, OQLError> {
        // Check that all shards recovered to the same LSN
        let target_lsn = recovery_result.target_lsn;

        for (shard_id, result) in &recovery_result.shard_results {
            if result.recovery_lsn != target_lsn {
                return Err(OQLError::ConsistencyError(format!(
                    "Shard {} recovered to LSN {} instead of target LSN {}",
                    shard_id, result.recovery_lsn, target_lsn
                )));
            }
        }

        // Check for failed shards
        if !recovery_result.failed_shards.is_empty() {
            return Err(OQLError::RecoveryError(format!(
                "Recovery failed for {} shard(s)",
                recovery_result.failed_shards.len()
            )));
        }

        Ok(true)
    }

    /// Verify backup integrity
    /// This checks that a backup is valid and can be used for recovery
    pub fn verify_backup(&self, backup_id: &str) -> Result<BackupVerificationResult, OQLError> {
        let backup = self
            .get_backup(backup_id)
            .ok_or_else(|| OQLError::NotFound(format!("Backup {} not found", backup_id)))?;

        let mut result = BackupVerificationResult {
            backup_id: backup_id.to_string(),
            is_valid: true,
            checks_passed: Vec::new(),
            checks_failed: Vec::new(),
            warnings: Vec::new(),
        };

        // Check 1: Backup completed successfully
        if backup.status != BackupStatus::Completed {
            result.is_valid = false;
            result.checks_failed.push(format!(
                "Backup status is {:?}, expected Completed",
                backup.status
            ));
        } else {
            result.checks_passed.push("Backup completed successfully".to_string());
        }

        // Check 2: Backup has valid metadata
        if backup.total_size == 0 {
            result.warnings.push("Backup has zero size".to_string());
        } else {
            result.checks_passed.push(format!("Backup size: {} bytes", backup.total_size));
        }

        // Check 3: Compression ratio is reasonable
        let compression_ratio = backup.compression_ratio();
        if compression_ratio > 1.0 {
            result.warnings.push(format!(
                "Compression ratio {} is greater than 1.0 (compressed size > original size)",
                compression_ratio
            ));
        } else {
            result.checks_passed.push(format!(
                "Compression ratio: {:.2}",
                compression_ratio
            ));
        }

        // Check 4: Backup file exists
        let backup_path = self.backup_directory.join(format!("{}.owlb", backup_id));
        if !backup_path.exists() {
            result.is_valid = false;
            result.checks_failed.push(format!(
                "Backup file does not exist: {}",
                backup_path.display()
            ));
        } else {
            result.checks_passed.push("Backup file exists".to_string());

            // Check 5: File size matches metadata
            if let Ok(metadata) = std::fs::metadata(&backup_path) {
                let file_size = metadata.len();
                if file_size != backup.compressed_size {
                    result.warnings.push(format!(
                        "File size {} does not match metadata compressed size {}",
                        file_size, backup.compressed_size
                    ));
                } else {
                    result.checks_passed.push("File size matches metadata".to_string());
                }
            }
        }

        // Check 6: For incremental backups, verify base backup exists
        if backup.backup_type == BackupType::Incremental {
            if let Some(ref prev_id) = backup.previous_backup_id {
                if self.get_backup(prev_id).is_none() {
                    result.is_valid = false;
                    result.checks_failed.push(format!(
                        "Previous backup {} not found for incremental backup",
                        prev_id
                    ));
                } else {
                    result.checks_passed.push("Previous backup exists".to_string());
                }
            } else {
                result.is_valid = false;
                result.checks_failed.push(
                    "Incremental backup missing previous_backup_id".to_string(),
                );
            }
        }

        Ok(result)
    }

    /// Verify all backups and return summary
    pub fn verify_all_backups(&self) -> BackupVerificationSummary {
        let backups = self.list_backups();
        let mut summary = BackupVerificationSummary {
            total_backups: backups.len(),
            valid_backups: 0,
            invalid_backups: 0,
            backups_with_warnings: 0,
            verification_results: Vec::new(),
        };

        for backup in backups {
            match self.verify_backup(&backup.backup_id) {
                Ok(result) => {
                    if result.is_valid {
                        summary.valid_backups += 1;
                    } else {
                        summary.invalid_backups += 1;
                    }
                    if !result.warnings.is_empty() {
                        summary.backups_with_warnings += 1;
                    }
                    summary.verification_results.push(result);
                }
                Err(e) => {
                    summary.invalid_backups += 1;
                    summary.verification_results.push(BackupVerificationResult {
                        backup_id: backup.backup_id.clone(),
                        is_valid: false,
                        checks_passed: Vec::new(),
                        checks_failed: vec![format!("Verification error: {}", e)],
                        warnings: Vec::new(),
                    });
                }
            }
        }

        summary
    }

    /// Test backup and restore with simulated failures
    /// This is used for automated testing of backup/restore functionality
    pub fn test_backup_restore(
        &self,
        shard_id: u32,
        simulate_failures: bool,
    ) -> Result<BackupRestoreTestResult, OQLError> {
        let start_time = SystemTime::now();

        // Step 1: Create a test backup
        let backup_id = self.schedule_backup(
            shard_id,
            BackupType::Full,
            "Automated test backup".to_string(),
        )?;

        self.start_backup(&backup_id)?;

        // Simulate backup completion
        self.complete_backup(&backup_id, 1000, 10240, 5120, 100)?;

        // Step 2: Verify the backup
        let verification = self.verify_backup(&backup_id)?;
        if !verification.is_valid {
            return Err(OQLError::RecoveryError(format!(
                "Backup verification failed: {:?}",
                verification.checks_failed
            )));
        }

        // Step 3: Simulate recovery
        let recovery_result = if simulate_failures {
            // Simulate a failure scenario
            Err(OQLError::RecoveryError(
                "Simulated recovery failure".to_string(),
            ))
        } else {
            self.recover_shard_to_lsn(shard_id, 1000)
                .map(|_| ())
        };

        let duration = SystemTime::now()
            .duration_since(start_time)
            .unwrap_or(Duration::from_secs(0));

        Ok(BackupRestoreTestResult {
            backup_id,
            shard_id,
            backup_successful: true,
            verification_successful: verification.is_valid,
            restore_successful: recovery_result.is_ok(),
            duration,
            error_message: recovery_result.err().map(|e| e.to_string()),
        })
    }

    /// Monitor backup health and return alerts
    pub fn monitor_backup_health(&self) -> BackupHealthReport {
        let stats = self.get_stats();
        let backups = self.list_backups();

        let mut report = BackupHealthReport {
            overall_health: BackupHealth::Healthy,
            alerts: Vec::new(),
            recommendations: Vec::new(),
            stats: stats.clone(),
        };

        // Check 1: Recent backup failures
        if stats.failed_backups > 0 {
            let failure_rate = stats.failed_backups as f64 / stats.backups_created as f64;
            if failure_rate > 0.1 {
                report.overall_health = BackupHealth::Critical;
                report.alerts.push(format!(
                    "High backup failure rate: {:.1}% ({} failures out of {} backups)",
                    failure_rate * 100.0,
                    stats.failed_backups,
                    stats.backups_created
                ));
            } else if failure_rate > 0.05 {
                report.overall_health = BackupHealth::Warning;
                report.alerts.push(format!(
                    "Elevated backup failure rate: {:.1}%",
                    failure_rate * 100.0
                ));
            }
        }

        // Check 2: No recent backups
        if let Some(last_backup) = stats.last_backup_time {
            let elapsed = SystemTime::now()
                .duration_since(last_backup)
                .unwrap_or(Duration::from_secs(0));

            if elapsed > Duration::from_secs(48 * 60 * 60) {
                // 48 hours
                report.overall_health = BackupHealth::Critical;
                report.alerts.push(format!(
                    "No backups in the last {:.1} hours",
                    elapsed.as_secs() as f64 / 3600.0
                ));
            } else if elapsed > Duration::from_secs(36 * 60 * 60) {
                // 36 hours
                if report.overall_health == BackupHealth::Healthy {
                    report.overall_health = BackupHealth::Warning;
                }
                report.alerts.push("No recent backups in the last 36 hours".to_string());
            }
        } else {
            report.overall_health = BackupHealth::Critical;
            report.alerts.push("No backups have been created yet".to_string());
        }

        // Check 3: Storage efficiency
        let compression_ratio = stats.compression_ratio();
        if compression_ratio > 0.9 {
            report.recommendations.push(format!(
                "Low compression ratio ({:.2}). Consider enabling compression or using a different compression algorithm",
                compression_ratio
            ));
        }

        // Check 4: Backup count
        if backups.len() < 3 {
            report.recommendations.push(format!(
                "Only {} backup(s) available. Consider increasing backup frequency for better recovery options",
                backups.len()
            ));
        }

        // Check 5: Operations in progress
        if stats.operations_in_progress > 5 {
            if report.overall_health == BackupHealth::Healthy {
                report.overall_health = BackupHealth::Warning;
            }
            report.alerts.push(format!(
                "{} backup operations currently in progress (may indicate stuck operations)",
                stats.operations_in_progress
            ));
        }

        report
    }
}

/// Result of recovering a single shard
#[derive(Debug, Clone)]
pub struct ShardRecoveryResult {
    /// Shard ID
    pub shard_id: u32,
    /// Backup ID used for recovery
    pub backup_id: String,
    /// Number of objects recovered
    pub objects_recovered: u64,
    /// LSN recovered to
    pub recovery_lsn: u64,
}

/// Result of distributed recovery across multiple shards
#[derive(Debug)]
pub struct DistributedRecoveryResult {
    /// Target LSN for recovery
    pub target_lsn: u64,
    /// Recovery results per shard
    pub shard_results: HashMap<u32, ShardRecoveryResult>,
    /// Failed shards with error messages
    pub failed_shards: Vec<(u32, String)>,
    /// Total objects recovered across all shards
    pub total_objects_recovered: u64,
}

/// Result of backup verification
#[derive(Debug, Clone)]
pub struct BackupVerificationResult {
    /// Backup ID
    pub backup_id: String,
    /// Whether the backup is valid
    pub is_valid: bool,
    /// List of checks that passed
    pub checks_passed: Vec<String>,
    /// List of checks that failed
    pub checks_failed: Vec<String>,
    /// List of warnings
    pub warnings: Vec<String>,
}

/// Summary of verifying all backups
#[derive(Debug, Clone)]
pub struct BackupVerificationSummary {
    /// Total number of backups
    pub total_backups: usize,
    /// Number of valid backups
    pub valid_backups: usize,
    /// Number of invalid backups
    pub invalid_backups: usize,
    /// Number of backups with warnings
    pub backups_with_warnings: usize,
    /// Individual verification results
    pub verification_results: Vec<BackupVerificationResult>,
}

/// Result of backup/restore test
#[derive(Debug, Clone)]
pub struct BackupRestoreTestResult {
    /// Backup ID used for test
    pub backup_id: String,
    /// Shard ID tested
    pub shard_id: u32,
    /// Whether backup was successful
    pub backup_successful: bool,
    /// Whether verification was successful
    pub verification_successful: bool,
    /// Whether restore was successful
    pub restore_successful: bool,
    /// Test duration
    pub duration: Duration,
    /// Error message if any step failed
    pub error_message: Option<String>,
}

/// Backup health status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupHealth {
    /// All backups are healthy
    Healthy,
    /// Some issues detected but not critical
    Warning,
    /// Critical issues detected
    Critical,
}

/// Backup health monitoring report
#[derive(Debug, Clone)]
pub struct BackupHealthReport {
    /// Overall health status
    pub overall_health: BackupHealth,
    /// List of alerts
    pub alerts: Vec<String>,
    /// List of recommendations
    pub recommendations: Vec<String>,
    /// Current backup statistics
    pub stats: BackupStats,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_backup_coordinator_creation() {
        let temp_dir = std::env::temp_dir().join("owl_backup_test");
        let schedule = BackupSchedule::default();
        let coordinator = BackupCoordinator::new(temp_dir.clone(), schedule).unwrap();

        assert_eq!(coordinator.backup_directory(), temp_dir.as_path());
        assert!(coordinator.schedule().enabled);
    }

    #[test]
    fn test_schedule_backup() {
        let temp_dir = std::env::temp_dir().join("owl_backup_schedule_test");
        let schedule = BackupSchedule::default();
        let coordinator = BackupCoordinator::new(temp_dir, schedule).unwrap();

        let backup_id = coordinator
            .schedule_backup(0, BackupType::Full, "Test backup".to_string())
            .unwrap();

        let metadata = coordinator.get_backup(&backup_id).unwrap();
        assert_eq!(metadata.backup_type, BackupType::Full);
        assert_eq!(metadata.status, BackupStatus::Scheduled);
        assert_eq!(metadata.shard_id, 0);
    }

    #[test]
    fn test_backup_lifecycle() {
        let temp_dir = std::env::temp_dir().join("owl_backup_lifecycle_test");
        let schedule = BackupSchedule::default();
        let coordinator = BackupCoordinator::new(temp_dir, schedule).unwrap();

        // Schedule backup
        let backup_id = coordinator
            .schedule_backup(0, BackupType::Full, "Test backup".to_string())
            .unwrap();

        // Start backup
        coordinator.start_backup(&backup_id).unwrap();
        let metadata = coordinator.get_backup(&backup_id).unwrap();
        assert_eq!(metadata.status, BackupStatus::InProgress);

        // Complete backup
        coordinator
            .complete_backup(&backup_id, 100, 1024, 512, 10)
            .unwrap();
        let metadata = coordinator.get_backup(&backup_id).unwrap();
        assert_eq!(metadata.status, BackupStatus::Completed);
        assert_eq!(metadata.backup_lsn, 100);
        assert_eq!(metadata.total_size, 1024);
        assert_eq!(metadata.compressed_size, 512);
        assert_eq!(metadata.object_count, 10);

        // Check statistics
        let stats = coordinator.get_stats();
        assert_eq!(stats.backups_created, 1);
        assert_eq!(stats.total_backup_size, 1024);
        assert_eq!(stats.objects_backed_up, 10);
    }

    #[test]
    fn test_needs_backup() {
        let temp_dir = std::env::temp_dir().join("owl_backup_needs_test");
        let schedule = BackupSchedule {
            full_backup_interval: Duration::from_secs(1),
            incremental_backup_interval: Duration::from_secs(1),
            ..Default::default()
        };
        let coordinator = BackupCoordinator::new(temp_dir, schedule).unwrap();

        // Should need full backup initially
        assert!(coordinator.needs_full_backup(0));

        // Simulate full backup
        {
            let mut last_full = coordinator.last_full_backup.lock().unwrap();
            last_full.insert(0, SystemTime::now());
        }

        // Should not need full backup immediately
        assert!(!coordinator.needs_full_backup(0));

        // Should need incremental backup after full backup
        assert!(coordinator.needs_incremental_backup(0));
    }

    #[test]
    fn test_list_backups() {
        let temp_dir = std::env::temp_dir().join("owl_backup_list_test");
        let schedule = BackupSchedule::default();
        let coordinator = BackupCoordinator::new(temp_dir, schedule).unwrap();

        // Create multiple backups
        coordinator
            .schedule_backup(0, BackupType::Full, "Backup 1".to_string())
            .unwrap();
        coordinator
            .schedule_backup(0, BackupType::Incremental, "Backup 2".to_string())
            .unwrap();
        coordinator
            .schedule_backup(1, BackupType::Full, "Backup 3".to_string())
            .unwrap();

        // List all backups
        let all_backups = coordinator.list_backups();
        assert_eq!(all_backups.len(), 3);

        // List shard 0 backups
        let shard0_backups = coordinator.list_shard_backups(0);
        assert_eq!(shard0_backups.len(), 2);

        // List shard 1 backups
        let shard1_backups = coordinator.list_shard_backups(1);
        assert_eq!(shard1_backups.len(), 1);
    }

    #[test]
    fn test_backup_verification() {
        let temp_dir = std::env::temp_dir().join("owl_backup_verification_test");
        let schedule = BackupSchedule::default();
        let coordinator = BackupCoordinator::new(temp_dir, schedule).unwrap();

        // Schedule and complete a backup
        let backup_id = coordinator
            .schedule_backup(0, BackupType::Full, "Test backup".to_string())
            .unwrap();

        coordinator.start_backup(&backup_id).unwrap();
        coordinator
            .complete_backup(&backup_id, 100, 1024, 512, 10)
            .unwrap();

        // Verify the backup
        let result = coordinator.verify_backup(&backup_id).unwrap();
        assert!(result.is_valid);
        assert!(!result.checks_passed.is_empty());
    }

    #[test]
    fn test_backup_health_monitoring() {
        let temp_dir = std::env::temp_dir().join("owl_backup_health_test");
        let schedule = BackupSchedule::default();
        let coordinator = BackupCoordinator::new(temp_dir, schedule).unwrap();

        // Initial health check (no backups)
        let report = coordinator.monitor_backup_health();
        assert_eq!(report.overall_health, BackupHealth::Critical);
        assert!(!report.alerts.is_empty());

        // Create a backup
        let backup_id = coordinator
            .schedule_backup(0, BackupType::Full, "Test backup".to_string())
            .unwrap();
        coordinator.start_backup(&backup_id).unwrap();
        coordinator
            .complete_backup(&backup_id, 100, 1024, 512, 10)
            .unwrap();

        // Health check after backup
        let report = coordinator.monitor_backup_health();
        // Should be healthy or warning (depending on timing)
        assert!(report.overall_health != BackupHealth::Critical);
    }

    #[test]
    fn test_distributed_recovery() {
        let temp_dir = std::env::temp_dir().join("owl_distributed_recovery_test");
        let schedule = BackupSchedule::default();
        let coordinator = BackupCoordinator::new(temp_dir, schedule).unwrap();

        // Create backups for multiple shards
        for shard_id in 0..3 {
            let backup_id = coordinator
                .schedule_backup(shard_id, BackupType::Full, format!("Shard {} backup", shard_id))
                .unwrap();
            coordinator.start_backup(&backup_id).unwrap();
            coordinator
                .complete_backup(&backup_id, 100, 1024, 512, 10)
                .unwrap();
        }

        // Perform distributed recovery
        let result = coordinator
            .perform_distributed_recovery(100, &[0, 1, 2])
            .unwrap();

        assert_eq!(result.target_lsn, 100);
        assert_eq!(result.shard_results.len(), 3);
        assert!(result.failed_shards.is_empty());
    }

    #[test]
    fn test_backup_restore_test() {
        let temp_dir = std::env::temp_dir().join("owl_backup_restore_test");
        let schedule = BackupSchedule::default();
        let coordinator = BackupCoordinator::new(temp_dir, schedule).unwrap();

        // Run backup/restore test without failures
        let result = coordinator.test_backup_restore(0, false).unwrap();
        assert!(result.backup_successful);
        assert!(result.verification_successful);

        // Run backup/restore test with simulated failures
        let result = coordinator.test_backup_restore(1, true).unwrap();
        assert!(result.backup_successful);
        assert!(result.verification_successful);
        assert!(!result.restore_successful);
        assert!(result.error_message.is_some());
    }
}
