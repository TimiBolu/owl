//! Statistics types for arena and co-location metrics

use std::fmt;

/// Arena statistics
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArenaStats {
    /// Total size of the arena in bytes
    pub total_size: usize,
    /// Used size in bytes
    pub used_size: usize,
    /// Free size in bytes
    pub free_size: usize,
    /// Number of objects allocated
    pub object_count: u64,
}

impl ArenaStats {
    /// Calculate utilization percentage
    pub fn utilization(&self) -> f64 {
        if self.total_size == 0 {
            0.0
        } else {
            (self.used_size as f64 / self.total_size as f64) * 100.0
        }
    }
}

impl fmt::Display for ArenaStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ArenaStats {{ total: {} KB, used: {} KB, free: {} KB, objects: {}, utilization: {:.1}% }}",
            self.total_size / 1024,
            self.used_size / 1024,
            self.free_size / 1024,
            self.object_count,
            self.utilization()
        )
    }
}

/// Co-location metrics
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColocationMetrics {
    /// Total co-location attempts
    pub total_attempts: usize,
    /// Successful co-locations (same or adjacent page)
    pub successful: usize,
    /// Failed co-locations
    pub failed: usize,
    /// Success rate as percentage
    pub success_rate: f64,
}

impl fmt::Display for ColocationMetrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ColocationMetrics {{ attempts: {}, successful: {}, failed: {}, rate: {:.1}% }}",
            self.total_attempts, self.successful, self.failed, self.success_rate
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arena_stats_utilization() {
        let stats = ArenaStats {
            total_size: 1024 * 1024,
            used_size: 512 * 1024,
            free_size: 512 * 1024,
            object_count: 100,
        };

        assert_eq!(stats.utilization(), 50.0);
    }

    #[test]
    fn test_arena_stats_display() {
        let stats = ArenaStats {
            total_size: 1024 * 1024,
            used_size: 512 * 1024,
            free_size: 512 * 1024,
            object_count: 100,
        };

        let display = format!("{}", stats);
        assert!(display.contains("1024 KB"));
        assert!(display.contains("512 KB"));
        assert!(display.contains("100"));
        assert!(display.contains("50.0%"));
    }

    #[test]
    fn test_colocation_metrics_display() {
        let metrics = ColocationMetrics {
            total_attempts: 100,
            successful: 80,
            failed: 20,
            success_rate: 80.0,
        };

        let display = format!("{}", metrics);
        assert!(display.contains("100"));
        assert!(display.contains("80"));
        assert!(display.contains("20"));
        assert!(display.contains("80.0%"));
    }
}
