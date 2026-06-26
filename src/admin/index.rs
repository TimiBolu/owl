use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::index_manager::{IndexBackendKind, IndexManager};
use crate::simulation::RealClock;
use crate::storage::create_storage;

pub struct IndexAdmin;

impl IndexAdmin {
    pub fn new(_config_path: &PathBuf) -> Result<Self> {
        Ok(Self)
    }

    pub fn rebuild(
        &self,
        db_path: &Path,
        size_mb: usize,
        catalog_path: Option<PathBuf>,
        backend: &str,
    ) -> Result<()> {
        let backend_kind = parse_backend_kind(backend)?;
        let catalog = catalog_path.unwrap_or_else(|| default_catalog_path(db_path));
        if !catalog.exists() {
            return Err(anyhow!(
                "index catalog '{}' does not exist",
                catalog.display()
            ));
        }

        let store = create_storage(
            db_path
                .to_str()
                .ok_or_else(|| anyhow!("invalid db path '{}'", db_path.display()))?,
            size_mb * 1024 * 1024,
        )?;

        let manager = IndexManager::load_catalog_with_backend(
            &catalog,
            Arc::new(RealClock::new()),
            backend_kind,
        )
        .map_err(anyhow::Error::msg)?;

        let report = manager
            .rebuild_from_store(store.as_ref())
            .map_err(anyhow::Error::msg)?;
        manager.save_catalog(&catalog).map_err(anyhow::Error::msg)?;

        println!("Index rebuild complete");
        println!("  DB: {}", db_path.display());
        println!("  Catalog: {}", catalog.display());
        println!("  Schemas processed: {}", report.schemas_processed);
        println!("  Documents scanned: {}", report.documents_scanned);
        println!("  Entries indexed: {}", report.entries_indexed);
        println!(
            "  Skipped (missing ObjRef): {}",
            report.documents_skipped_missing_objref
        );
        Ok(())
    }

    pub fn list(&self, catalog_path: &Path) -> Result<()> {
        let manager = IndexManager::load_catalog_with_backend(
            catalog_path,
            Arc::new(RealClock::new()),
            IndexBackendKind::InMemory,
        )
        .map_err(anyhow::Error::msg)?;
        let defs = manager
            .list_index_definitions()
            .map_err(anyhow::Error::msg)?;
        if defs.is_empty() {
            println!("No indexes in '{}'", catalog_path.display());
            return Ok(());
        }
        println!("Indexes in '{}':", catalog_path.display());
        for d in defs {
            println!(
                "  {} | schema={} fields={:?} unique={} entries={}",
                d.name,
                d.schema,
                d.fields,
                d.unique,
                d.entries.len()
            );
        }
        Ok(())
    }
}

fn parse_backend_kind(input: &str) -> Result<IndexBackendKind> {
    match input.to_ascii_lowercase().as_str() {
        "zig" => Ok(IndexBackendKind::Zig),
        "memory" | "inmemory" => Ok(IndexBackendKind::InMemory),
        other => Err(anyhow!(
            "unsupported backend '{}'; use zig or memory",
            other
        )),
    }
}

fn default_catalog_path(db_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.indexes.json", db_path.display()))
}
