// Index Scan Operator
// Efficient index-based scanning for queries with indexed predicates

use crate::document::Document;
use crate::error::{OQLError, OQLResult};
use crate::execution::operator::{DataChunk, PhysicalOperatorExec};
use crate::execution::scan::FilterExec;
use crate::index_manager::{IndexBackendKind, IndexManager};
use crate::planner::types::FilterCondition;
use crate::schema::DocumentSystemField;
use crate::simulation::RealClock;
use crate::storage::{
    ScalarIndexBound, ScalarIndexCursor, ScalarIndexDefinition, ScalarIndexDirection,
    ScalarIndexRange, Storage,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use wyhash::wyhash;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexBackendAccess {
    NativeTyped,
    CompatibilityHashed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexAccessPath {
    pub backend: IndexBackendAccess,
    pub schema: String,
    pub index_name: String,
    pub key_fields: Vec<String>,
    pub rows_loaded: usize,
    pub limit: Option<usize>,
    pub projected_fields: Vec<String>,
    pub continued_from_cursor: bool,
    pub direction: ScalarIndexDirection,
}

/// Index scan operator
///
/// Uses an index to efficiently scan documents matching a condition.
/// Much faster than sequential scan for selective queries.
pub struct IndexScanExec {
    schema: String,
    index_name: String,
    prefilter_index: Option<String>,
    key_conditions: Vec<FilterCondition>,
    storage: Option<Storage>,
    index_manager: Option<Arc<IndexManager>>,
    documents: VecDeque<Document>,
    native_scan: Option<NativeIndexScanState>,
    limit: Option<usize>,
    projection: Option<Vec<String>>,
    cursor: Option<ScalarIndexCursor>,
    next_cursor: Option<ScalarIndexCursor>,
    access_path: Option<IndexAccessPath>,
    direction: ScalarIndexDirection,
    metrics: IndexScanMetrics,
}

pub struct IndexUnionExec {
    inputs: Vec<Box<dyn PhysicalOperatorExec>>,
    documents: VecDeque<Document>,
    order_fields: Vec<String>,
    direction: ScalarIndexDirection,
    limit: Option<usize>,
}

impl IndexUnionExec {
    pub fn new(
        inputs: Vec<Box<dyn PhysicalOperatorExec>>,
        order_fields: Vec<String>,
        direction: ScalarIndexDirection,
        limit: Option<usize>,
    ) -> Self {
        Self {
            inputs,
            documents: VecDeque::new(),
            order_fields,
            direction,
            limit,
        }
    }

    fn compare_documents(&self, left: &Document, right: &Document) -> std::cmp::Ordering {
        fn value(document: &Document, field: &str) -> crate::document::Value {
            DocumentSystemField::from_name(field)
                .map(|system| system.value(document))
                .or_else(|| document.fields.get(field).cloned())
                .unwrap_or(crate::document::Value::Null)
        }

        fn compare(
            left: &crate::document::Value,
            right: &crate::document::Value,
        ) -> std::cmp::Ordering {
            use crate::document::Value;
            match (left, right) {
                (Value::Integer(left), Value::Integer(right)) => left.cmp(right),
                (Value::Float(left), Value::Float(right)) => {
                    left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
                }
                (Value::Integer(left), Value::Float(right)) => (*left as f64)
                    .partial_cmp(right)
                    .unwrap_or(std::cmp::Ordering::Equal),
                (Value::Float(left), Value::Integer(right)) => left
                    .partial_cmp(&(*right as f64))
                    .unwrap_or(std::cmp::Ordering::Equal),
                (Value::String(left), Value::String(right)) => left.cmp(right),
                (Value::DateTime(left), Value::DateTime(right)) => left.cmp(right),
                (Value::Boolean(left), Value::Boolean(right)) => left.cmp(right),
                (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
                (Value::Null, _) => std::cmp::Ordering::Less,
                (_, Value::Null) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            }
        }

        for field in &self.order_fields {
            let ordering = compare(&value(left, field), &value(right, field));
            let ordering = match self.direction {
                ScalarIndexDirection::Ascending => ordering,
                ScalarIndexDirection::Descending => ordering.reverse(),
            };
            if ordering != std::cmp::Ordering::Equal {
                return ordering;
            }
        }
        left.schema
            .cmp(&right.schema)
            .then_with(|| left.id.cmp(&right.id))
    }
}

impl PhysicalOperatorExec for IndexUnionExec {
    fn open(&mut self, storage: Storage) -> OQLResult<()> {
        let mut by_identity = HashMap::new();
        for input in &mut self.inputs {
            input.open(storage.clone())?;
            while let Some(chunk) = input.next_batch(crate::execution::DEFAULT_CHUNK_ROWS)? {
                for document in chunk.into_documents() {
                    by_identity
                        .entry((document.schema.clone(), document.id.clone()))
                        .or_insert(document);
                }
            }
            input.close()?;
        }
        let mut documents = by_identity.into_values().collect::<Vec<_>>();
        documents.sort_by(|left, right| self.compare_documents(left, right));
        if let Some(limit) = self.limit {
            documents.truncate(limit);
        }
        self.documents = documents.into();
        Ok(())
    }

    fn next(&mut self) -> OQLResult<Option<Document>> {
        Ok(self.documents.pop_front())
    }

    fn close(&mut self) -> OQLResult<()> {
        self.documents.clear();
        Ok(())
    }

    fn reset(&mut self) -> OQLResult<()> {
        self.documents.clear();
        Ok(())
    }
}

struct NativeIndexScanState {
    definition: ScalarIndexDefinition,
    range: ScalarIndexRange,
    prefiltered_refs: Option<HashSet<crate::document::ObjRef>>,
    pending: VecDeque<(crate::document::ObjRef, Vec<u8>)>,
    next_page_cursor: Option<ScalarIndexCursor>,
    started: bool,
    exhausted: bool,
    rows_emitted: usize,
}

#[derive(Default)]
struct IndexScanMetrics {
    index_pages_requested: u64,
    index_entries_fetched: u64,
    index_entries_examined: u64,
    prefilter_candidates: u64,
    rejected_by_prefilter: u64,
    rejected_by_key: u64,
    rejected_by_residual: u64,
    records_decoded: u64,
}

impl IndexScanExec {
    /// Create a new index scan operator
    pub fn new(schema: String, index_name: String, key_conditions: Vec<FilterCondition>) -> Self {
        Self {
            schema,
            index_name,
            prefilter_index: None,
            key_conditions,
            storage: None,
            index_manager: None,
            documents: VecDeque::new(),
            native_scan: None,
            limit: None,
            projection: None,
            cursor: None,
            next_cursor: None,
            access_path: None,
            direction: ScalarIndexDirection::Ascending,
            metrics: IndexScanMetrics::default(),
        }
    }

    /// Create a new index scan operator with an injected index manager.
    pub fn new_with_index_manager(
        schema: String,
        index_name: String,
        key_conditions: Vec<FilterCondition>,
        index_manager: Arc<IndexManager>,
    ) -> Self {
        Self {
            schema,
            index_name,
            prefilter_index: None,
            key_conditions,
            storage: None,
            index_manager: Some(index_manager),
            documents: VecDeque::new(),
            native_scan: None,
            limit: None,
            projection: None,
            cursor: None,
            next_cursor: None,
            access_path: None,
            direction: ScalarIndexDirection::Ascending,
            metrics: IndexScanMetrics::default(),
        }
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn with_prefilter_index(mut self, name: String) -> Self {
        self.prefilter_index = Some(name);
        self
    }

    pub fn with_projection(mut self, fields: Vec<String>) -> Self {
        self.projection = Some(fields);
        self
    }

    pub fn with_cursor(mut self, cursor: ScalarIndexCursor) -> Self {
        self.cursor = Some(cursor);
        self
    }

    pub fn with_direction(mut self, direction: ScalarIndexDirection) -> Self {
        self.direction = direction;
        self
    }

    pub fn next_cursor(&self) -> Option<&ScalarIndexCursor> {
        self.next_cursor.as_ref()
    }

    pub fn access_path(&self) -> Option<&IndexAccessPath> {
        self.access_path.as_ref()
    }

    /// Extract key range from conditions
    /// Returns (start_key, end_key) for range scan
    pub fn extract_key_range(&self) -> OQLResult<(u64, u64)> {
        // For now, support simple equality and range conditions
        // This is a simplified implementation

        if self.key_conditions.is_empty() {
            // No conditions - full index scan
            return Ok((0, u64::MAX));
        }

        // Extract first condition (simplified)
        match &self.key_conditions[0] {
            FilterCondition::Comparison {
                left: _,
                operator,
                right,
            } => {
                // Try to extract literal value
                if let crate::parser::Expression::Literal(lit) = right {
                    let key = Self::literal_to_index_key(lit)
                        .ok_or_else(|| OQLError::generic("Unsupported key type for index scan"))?;

                    // Map operator to range
                    use crate::parser::ComparisonOperator;
                    match operator {
                        ComparisonOperator::Equal => Ok((key, key)),
                        ComparisonOperator::LessThan if Self::literal_supports_ordering(lit) => {
                            Ok((0, key.saturating_sub(1)))
                        }
                        ComparisonOperator::LessEqual if Self::literal_supports_ordering(lit) => {
                            Ok((0, key))
                        }
                        ComparisonOperator::GreaterThan if Self::literal_supports_ordering(lit) => {
                            Ok((key.saturating_add(1), u64::MAX))
                        }
                        ComparisonOperator::GreaterEqual
                            if Self::literal_supports_ordering(lit) =>
                        {
                            Ok((key, u64::MAX))
                        }
                        _ => Err(OQLError::generic("Unsupported operator for index scan")),
                    }
                } else {
                    Err(OQLError::generic(
                        "Index scan requires literal comparison value",
                    ))
                }
            }
            _ => Err(OQLError::generic(
                "Unsupported condition type for index scan",
            )),
        }
    }

    /// Load documents using index
    fn load_documents_from_index(&mut self) -> OQLResult<()> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| OQLError::generic("Storage not initialized"))?;

        let index_manager = self
            .index_manager
            .as_ref()
            .ok_or_else(|| OQLError::generic("Index manager not initialized"))?;

        // Get indexes for this schema
        let indexes = index_manager
            .get_indexes(&self.schema)
            .map_err(|e| OQLError::generic(&format!("Failed to get indexes: {}", e)))?;

        // Find the index we want to use
        let index = indexes
            .iter()
            .find(|idx| idx.name == self.index_name)
            .ok_or_else(|| OQLError::generic(&format!("Index '{}' not found", self.index_name)))?;

        // Extract key range from conditions
        let (start_key, end_key) = self.extract_key_range()?;

        // Perform range query on index
        let objrefs = index
            .range_query(start_key, end_key)
            .map_err(|e| OQLError::generic(&format!("Index range query failed: {}", e)))?;

        // Build a one-pass lookup map from ObjRef offset -> Document.
        // This avoids returning wrong documents when multiple rows exist.
        let mut by_offset: HashMap<u64, Document> = HashMap::new();
        let all_docs = storage.scan_documents(&self.schema)?;
        for doc in all_docs {
            if let Some(objref) = storage.get_objref(&self.schema, &doc.id)? {
                by_offset.insert(objref.offset(), doc);
            }
        }

        // Load documents from storage using ObjRef offset lookup.
        self.documents.clear();
        for objref in objrefs {
            if let Some(doc) = by_offset.remove(&objref.offset()) {
                self.documents.push_back(doc);
            }
        }

        if let Some(limit) = self.limit {
            self.documents.truncate(limit);
        }
        if let Some(fields) = &self.projection {
            let selected = fields.iter().collect::<std::collections::HashSet<_>>();
            for document in &mut self.documents {
                document.fields.retain(|name, _| selected.contains(name));
            }
        }
        self.access_path = Some(IndexAccessPath {
            backend: IndexBackendAccess::CompatibilityHashed,
            schema: self.schema.clone(),
            index_name: self.index_name.clone(),
            key_fields: Vec::new(),
            rows_loaded: self.documents.len(),
            limit: self.limit,
            projected_fields: self.projection.clone().unwrap_or_default(),
            continued_from_cursor: false,
            direction: self.direction,
        });

        Ok(())
    }

    fn initialize_native_index(&mut self) -> OQLResult<bool> {
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| OQLError::generic("Storage not initialized"))?
            .clone();
        let definitions = storage.list_scalar_indexes(Some(&self.schema))?;
        let Some(definition) = definitions
            .iter()
            .find(|definition| definition.name == self.index_name)
        else {
            return Ok(false);
        };
        let prefiltered_refs = if let Some(prefilter_name) = &self.prefilter_index {
            let prefilter_definition = definitions
                .iter()
                .find(|definition| definition.name == *prefilter_name)
                .ok_or_else(|| {
                    OQLError::storage_error("planned scalar prefilter index is missing", None)
                })?;
            let mut prefilter_range = self.extract_typed_index_range(prefilter_definition)?;
            let ordered_covers_filter =
                Self::conditions_covered_by_fields(&self.key_conditions, &definition.fields);
            prefilter_range.limit = if ordered_covers_filter {
                self.limit.map(|limit| limit.saturating_add(1))
            } else {
                None
            };
            prefilter_range.after = None;
            prefilter_range.direction = ScalarIndexDirection::Ascending;
            self.metrics.index_pages_requested += 1;
            let page = storage
                .scan_scalar_index(&self.schema, prefilter_name, &prefilter_range)?
                .ok_or_else(|| {
                    OQLError::storage_error("planned scalar prefilter index is unavailable", None)
                })?;
            self.metrics.prefilter_candidates = page.object_refs.len() as u64;
            if ordered_covers_filter && page.next_cursor.is_some() {
                None
            } else {
                Some(page.object_refs.into_iter().collect::<HashSet<_>>())
            }
        } else {
            None
        };
        let mut range = self.extract_typed_index_range(definition)?;
        if self.cursor.as_ref().is_some_and(|cursor| {
            cursor.version != 2
                || cursor.schema != self.schema
                || cursor.index_name != self.index_name
                || cursor.direction != self.direction
        }) {
            return Err(OQLError::generic(
                "index cursor does not belong to the selected index",
            ));
        }
        range.after = self.cursor.clone();
        range.direction = self.direction;
        self.access_path = Some(IndexAccessPath {
            backend: IndexBackendAccess::NativeTyped,
            schema: self.schema.clone(),
            index_name: self.index_name.clone(),
            key_fields: definition.fields.clone(),
            rows_loaded: 0,
            limit: self.limit,
            projected_fields: self.projection.clone().unwrap_or_default(),
            continued_from_cursor: self.cursor.is_some(),
            direction: self.direction,
        });
        self.native_scan = Some(NativeIndexScanState {
            definition: definition.clone(),
            range,
            prefiltered_refs,
            pending: VecDeque::new(),
            next_page_cursor: None,
            started: false,
            exhausted: false,
            rows_emitted: 0,
        });
        Ok(true)
    }

    fn load_next_native_page(
        &mut self,
        state: &mut NativeIndexScanState,
        row_goal: usize,
    ) -> OQLResult<()> {
        if state.started {
            let Some(cursor) = state.next_page_cursor.take() else {
                state.exhausted = true;
                return Ok(());
            };
            state.range.after = Some(cursor);
        }

        // Fetch enough entries to amortize tree traversal while keeping
        // selective top-K queries from materializing a fixed 1,024-entry page.
        state.range.limit = Some(row_goal.saturating_mul(4).clamp(64, 1024));
        self.metrics.index_pages_requested += 1;
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| OQLError::generic("Storage not initialized"))?;
        let page = storage
            .scan_scalar_index(&self.schema, &self.index_name, &state.range)?
            .ok_or_else(|| {
                OQLError::storage_error(
                    "native scalar index became unavailable during execution",
                    None,
                )
            })?;
        state.started = true;
        if page.object_refs.len() != page.encoded_keys.len() {
            return Err(OQLError::storage_error(
                "native scalar index returned misaligned keys and references",
                None,
            ));
        }
        self.metrics.index_entries_fetched = self
            .metrics
            .index_entries_fetched
            .saturating_add(page.object_refs.len() as u64);
        state
            .pending
            .extend(page.object_refs.into_iter().zip(page.encoded_keys));
        state.next_page_cursor = page.next_cursor;
        if state.pending.is_empty() && state.next_page_cursor.is_none() {
            state.exhausted = true;
        }
        Ok(())
    }

    fn next_native_batch(
        &mut self,
        state: &mut NativeIndexScanState,
        target_rows: usize,
    ) -> OQLResult<Option<DataChunk>> {
        let remaining_limit = self
            .limit
            .map(|limit| limit.saturating_sub(state.rows_emitted))
            .unwrap_or(usize::MAX);
        if remaining_limit == 0 || state.exhausted {
            return Ok(None);
        }

        let row_goal = target_rows.max(1).min(remaining_limit);
        let mut output = DataChunk::with_capacity(row_goal);
        while output.len() < row_goal {
            // Checkpoint-backed projection retains record blobs until the
            // storage batch is decoded. Keep that working set bounded; larger
            // result chunks are assembled from several storage batches.
            let candidate_goal = (row_goal - output.len()).min(256);
            let mut candidates = Vec::with_capacity(candidate_goal);
            while candidates.len() < candidate_goal {
                if state.pending.is_empty() {
                    self.load_next_native_page(state, candidate_goal - candidates.len())?;
                    if state.pending.is_empty() {
                        if state.exhausted {
                            break;
                        }
                        continue;
                    }
                }

                let (object_ref, encoded_key) = state
                    .pending
                    .pop_front()
                    .expect("native page checked non-empty");
                self.metrics.index_entries_examined += 1;
                if state
                    .prefiltered_refs
                    .as_ref()
                    .is_some_and(|references| !references.contains(&object_ref))
                {
                    self.metrics.rejected_by_prefilter += 1;
                    continue;
                }

                let key_match = self.key_conditions_match(&state.definition, &encoded_key)?;
                if key_match == Some(false) {
                    self.metrics.rejected_by_key += 1;
                    continue;
                }
                candidates.push((object_ref, encoded_key, key_match));
            }

            if candidates.is_empty() {
                if state.exhausted {
                    break;
                }
                continue;
            }

            let storage = self
                .storage
                .as_ref()
                .ok_or_else(|| OQLError::generic("Storage not initialized"))?;
            let object_refs = candidates
                .iter()
                .map(|(object_ref, _, _)| *object_ref)
                .collect::<Vec<_>>();
            let documents = match &self.projection {
                Some(fields) => storage.navigate_references_projection(&object_refs, fields)?,
                None => storage.navigate_references(&object_refs)?,
            };
            if documents.len() != candidates.len() {
                return Err(OQLError::storage_error(
                    "batch reference navigation returned a misaligned result",
                    None,
                ));
            }
            self.metrics.records_decoded = self
                .metrics
                .records_decoded
                .saturating_add(candidates.len() as u64);

            for ((object_ref, encoded_key, key_match), document) in
                candidates.into_iter().zip(documents)
            {
                let document = document.ok_or_else(|| {
                    OQLError::storage_error("native scalar index references a missing object", None)
                })?;
                if document.schema != self.schema {
                    return Err(OQLError::storage_error(
                        "native scalar index references the wrong collection",
                        None,
                    ));
                }

                let matches = match key_match {
                    Some(matches) => matches,
                    None => {
                        let mut matches = true;
                        for condition in &self.key_conditions {
                            if !FilterExec::matches(&document, condition)? {
                                matches = false;
                                break;
                            }
                        }
                        matches
                    }
                };
                if !matches {
                    self.metrics.rejected_by_residual += 1;
                    continue;
                }

                output.push(document);
                state.rows_emitted += 1;
                if let Some(access_path) = &mut self.access_path {
                    access_path.rows_loaded = state.rows_emitted;
                }
                if self.limit.is_some_and(|limit| state.rows_emitted >= limit) {
                    let has_more = !state.pending.is_empty() || state.next_page_cursor.is_some();
                    self.next_cursor = has_more.then(|| ScalarIndexCursor {
                        version: 2,
                        schema: self.schema.clone(),
                        index_name: self.index_name.clone(),
                        key: encoded_key,
                        object_ref,
                        direction: self.direction,
                    });
                    break;
                }
            }
        }

        Ok((!output.is_empty()).then_some(output))
    }

    fn extract_typed_index_range(
        &self,
        definition: &ScalarIndexDefinition,
    ) -> OQLResult<ScalarIndexRange> {
        if self.key_conditions.is_empty() {
            return Ok(ScalarIndexRange::default());
        }
        let mut comparisons = Vec::new();
        for condition in &self.key_conditions {
            Self::collect_comparisons(condition, &mut comparisons);
        }

        if definition.fields.len() > 1 {
            let mut prefix = Vec::with_capacity(definition.fields.len());
            for field in &definition.fields {
                let equality = comparisons
                    .iter()
                    .find_map(|(candidate, operator, literal)| {
                        (candidate == field
                            && matches!(operator, crate::parser::ComparisonOperator::Equal))
                        .then_some(*literal)
                    });
                let Some(literal) = equality else { break };
                prefix.push(Self::literal_to_value(literal));
            }
            if prefix.is_empty() {
                return Err(OQLError::generic(
                    "composite index scan must constrain its leading field",
                ));
            }
            if prefix.len() == definition.fields.len() {
                let bound = ScalarIndexBound {
                    values: prefix,
                    inclusive: true,
                    prefix_end: false,
                };
                return Ok(ScalarIndexRange {
                    lower: Some(bound.clone()),
                    upper: Some(bound),
                    limit: None,
                    after: None,
                    direction: ScalarIndexDirection::Ascending,
                });
            }

            let range_field = &definition.fields[prefix.len()];
            let has_suffix = prefix.len() + 1 < definition.fields.len();
            let mut range = ScalarIndexRange::default();
            let mut found_range = false;
            for (candidate, operator, literal) in &comparisons {
                if candidate != range_field {
                    continue;
                }
                found_range = true;
                let mut values = prefix.clone();
                values.push(Self::literal_to_value(literal));
                use crate::parser::ComparisonOperator;
                match operator {
                    ComparisonOperator::GreaterEqual => Self::tighten_lower(
                        &mut range.lower,
                        ScalarIndexBound {
                            values,
                            inclusive: true,
                            prefix_end: false,
                        },
                    )?,
                    ComparisonOperator::GreaterThan => Self::tighten_lower(
                        &mut range.lower,
                        ScalarIndexBound {
                            values,
                            inclusive: true,
                            prefix_end: has_suffix,
                        },
                    )?,
                    ComparisonOperator::LessThan => Self::tighten_upper(
                        &mut range.upper,
                        ScalarIndexBound {
                            values,
                            inclusive: false,
                            prefix_end: false,
                        },
                    )?,
                    ComparisonOperator::LessEqual => Self::tighten_upper(
                        &mut range.upper,
                        ScalarIndexBound {
                            values,
                            inclusive: !has_suffix,
                            prefix_end: has_suffix,
                        },
                    )?,
                    _ => {
                        return Err(OQLError::generic(
                            "composite range field requires an ordered comparison",
                        ));
                    }
                }
            }
            if found_range {
                let prefix_lower = ScalarIndexBound {
                    values: prefix.clone(),
                    inclusive: true,
                    prefix_end: false,
                };
                let prefix_upper = ScalarIndexBound {
                    values: prefix,
                    inclusive: false,
                    prefix_end: true,
                };
                Self::tighten_lower(&mut range.lower, prefix_lower)?;
                Self::tighten_upper(&mut range.upper, prefix_upper)?;
                return Ok(range);
            }
            return Ok(ScalarIndexRange {
                lower: Some(ScalarIndexBound {
                    values: prefix.clone(),
                    inclusive: true,
                    prefix_end: false,
                }),
                upper: Some(ScalarIndexBound {
                    values: prefix,
                    inclusive: false,
                    prefix_end: true,
                }),
                limit: None,
                after: None,
                direction: ScalarIndexDirection::Ascending,
            });
        }

        let field = definition
            .fields
            .first()
            .ok_or_else(|| OQLError::generic("scalar index has no fields"))?;
        let mut range = ScalarIndexRange::default();
        let mut found = false;
        for (candidate, operator, literal) in comparisons {
            if &candidate != field {
                continue;
            }
            found = true;
            let values = vec![Self::literal_to_value(literal)];
            use crate::parser::ComparisonOperator;
            match operator {
                ComparisonOperator::Equal => {
                    let bound = ScalarIndexBound {
                        values,
                        inclusive: true,
                        prefix_end: false,
                    };
                    Self::tighten_lower(&mut range.lower, bound.clone())?;
                    Self::tighten_upper(&mut range.upper, bound)?;
                }
                ComparisonOperator::GreaterThan | ComparisonOperator::GreaterEqual => {
                    Self::tighten_lower(
                        &mut range.lower,
                        ScalarIndexBound {
                            values,
                            inclusive: matches!(operator, ComparisonOperator::GreaterEqual),
                            prefix_end: false,
                        },
                    )?;
                }
                ComparisonOperator::LessThan | ComparisonOperator::LessEqual => {
                    Self::tighten_upper(
                        &mut range.upper,
                        ScalarIndexBound {
                            values,
                            inclusive: matches!(operator, ComparisonOperator::LessEqual),
                            prefix_end: false,
                        },
                    )?;
                }
                _ => {
                    return Err(OQLError::generic(
                        "operator is not supported by native ordered index scan",
                    ));
                }
            }
        }
        if !found {
            return Err(OQLError::generic(
                "index conditions do not constrain the indexed field",
            ));
        }
        Ok(range)
    }

    fn tighten_lower(
        current: &mut Option<ScalarIndexBound>,
        candidate: ScalarIndexBound,
    ) -> OQLResult<()> {
        let Some(existing) = current else {
            *current = Some(candidate);
            return Ok(());
        };
        let existing_key = Self::encoded_bound(existing)?;
        let candidate_key = Self::encoded_bound(&candidate)?;
        if candidate_key > existing_key {
            *existing = candidate;
        } else if candidate_key == existing_key {
            existing.inclusive &= candidate.inclusive;
        }
        Ok(())
    }

    fn tighten_upper(
        current: &mut Option<ScalarIndexBound>,
        candidate: ScalarIndexBound,
    ) -> OQLResult<()> {
        let Some(existing) = current else {
            *current = Some(candidate);
            return Ok(());
        };
        let existing_key = Self::encoded_bound(existing)?;
        let candidate_key = Self::encoded_bound(&candidate)?;
        if candidate_key < existing_key {
            *existing = candidate;
        } else if candidate_key == existing_key {
            existing.inclusive &= candidate.inclusive;
        }
        Ok(())
    }

    fn encoded_bound(bound: &ScalarIndexBound) -> OQLResult<Vec<u8>> {
        let key = crate::storage_engine::TypedKey::encode(&bound.values)?;
        if bound.prefix_end {
            crate::storage_engine::TypedKey::prefix_successor(key).ok_or_else(|| {
                OQLError::storage_error("typed index prefix has no finite successor", None)
            })
        } else {
            Ok(key)
        }
    }

    fn collect_comparisons<'a>(
        condition: &'a FilterCondition,
        output: &mut Vec<(
            String,
            &'a crate::parser::ComparisonOperator,
            &'a crate::parser::Literal,
        )>,
    ) {
        match condition {
            FilterCondition::Comparison {
                left: crate::parser::Expression::FieldAccess(field),
                operator,
                right: crate::parser::Expression::Literal(literal),
            } => output.push((field.path.join("."), operator, literal)),
            FilterCondition::And(left, right) => {
                Self::collect_comparisons(left, output);
                Self::collect_comparisons(right, output);
            }
            _ => {}
        }
    }

    fn key_conditions_match(
        &self,
        definition: &ScalarIndexDefinition,
        encoded_key: &[u8],
    ) -> OQLResult<Option<bool>> {
        let mut comparisons = Vec::new();
        for condition in &self.key_conditions {
            Self::collect_comparisons(condition, &mut comparisons);
        }
        let enforced_prefix = definition
            .fields
            .iter()
            .map_while(|field| {
                comparisons
                    .iter()
                    .find(|(candidate, operator, _)| {
                        candidate == field
                            && matches!(operator, crate::parser::ComparisonOperator::Equal)
                    })
                    .map(|(_, _, literal)| (field.as_str(), *literal))
            })
            .collect::<Vec<_>>();
        let mut result = Some(true);
        for condition in &self.key_conditions {
            result = Self::combine_key_and(
                result,
                Self::key_condition_match_encoded(
                    condition,
                    &definition.fields,
                    &enforced_prefix,
                    encoded_key,
                )?,
            );
            if result == Some(false) {
                break;
            }
        }
        Ok(result)
    }

    fn conditions_covered_by_fields(conditions: &[FilterCondition], fields: &[String]) -> bool {
        conditions
            .iter()
            .all(|condition| Self::condition_covered_by_fields(condition, fields))
    }

    fn condition_covered_by_fields(condition: &FilterCondition, fields: &[String]) -> bool {
        match condition {
            FilterCondition::Comparison { left, right, .. } => {
                [left, right].iter().all(|expression| match expression {
                    crate::parser::Expression::FieldAccess(field) => {
                        field.path.len() == 1 && fields.contains(&field.path[0])
                    }
                    crate::parser::Expression::Literal(_) => true,
                    _ => false,
                })
            }
            FilterCondition::And(left, right) | FilterCondition::Or(left, right) => {
                Self::condition_covered_by_fields(left, fields)
                    && Self::condition_covered_by_fields(right, fields)
            }
            FilterCondition::Not(inner) => Self::condition_covered_by_fields(inner, fields),
        }
    }

    fn key_condition_match_encoded(
        condition: &FilterCondition,
        fields: &[String],
        enforced_prefix: &[(&str, &crate::parser::Literal)],
        encoded_key: &[u8],
    ) -> OQLResult<Option<bool>> {
        use crate::planner::types::FilterCondition;
        match condition {
            FilterCondition::Comparison {
                left,
                operator,
                right,
            } => {
                if let (
                    crate::parser::Expression::FieldAccess(field),
                    crate::parser::Expression::Literal(literal),
                ) = (left, right)
                {
                    if field.path.len() == 1
                        && matches!(operator, crate::parser::ComparisonOperator::Equal)
                        && enforced_prefix
                            .iter()
                            .any(|(name, value)| *name == field.path[0] && *value == literal)
                    {
                        return Ok(Some(true));
                    }
                    if field.path.len() == 1 {
                        if let Some(position) =
                            fields.iter().position(|name| name == &field.path[0])
                        {
                            let left = crate::storage_engine::TypedKey::decode_component(
                                encoded_key,
                                position,
                            )?;
                            let right = Self::literal_to_value(literal);
                            return Ok(Some(FilterExec::compare_values(&left, operator, &right)));
                        }
                    }
                }
                Ok(None)
            }
            FilterCondition::And(left, right) => Ok(Self::combine_key_and(
                Self::key_condition_match_encoded(left, fields, enforced_prefix, encoded_key)?,
                Self::key_condition_match_encoded(right, fields, enforced_prefix, encoded_key)?,
            )),
            FilterCondition::Or(left, right) => {
                match (
                    Self::key_condition_match_encoded(left, fields, enforced_prefix, encoded_key)?,
                    Self::key_condition_match_encoded(right, fields, enforced_prefix, encoded_key)?,
                ) {
                    (Some(true), _) | (_, Some(true)) => Ok(Some(true)),
                    (Some(false), Some(false)) => Ok(Some(false)),
                    _ => Ok(None),
                }
            }
            FilterCondition::Not(inner) => {
                Ok(
                    Self::key_condition_match_encoded(inner, fields, enforced_prefix, encoded_key)?
                        .map(|value| !value),
                )
            }
        }
    }

    fn combine_key_and(left: Option<bool>, right: Option<bool>) -> Option<bool> {
        match (left, right) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (Some(true), Some(true)) => Some(true),
            _ => None,
        }
    }

    fn literal_to_value(literal: &crate::parser::Literal) -> crate::document::Value {
        match literal {
            crate::parser::Literal::String(value) => crate::document::Value::String(value.clone()),
            crate::parser::Literal::Integer(value) => crate::document::Value::Integer(*value),
            crate::parser::Literal::Float(value) => crate::document::Value::Float(*value),
            crate::parser::Literal::Boolean(value) => crate::document::Value::Boolean(*value),
            crate::parser::Literal::Null => crate::document::Value::Null,
        }
    }

    fn literal_to_index_key(lit: &crate::parser::Literal) -> Option<u64> {
        match lit {
            crate::parser::Literal::Integer(i) => Some(*i as u64),
            crate::parser::Literal::Float(f) => Some(f.to_bits()),
            crate::parser::Literal::Boolean(b) => Some(u64::from(*b)),
            crate::parser::Literal::String(s) => Some(wyhash(s.as_bytes(), 0)),
            crate::parser::Literal::Null => None,
        }
    }

    fn literal_supports_ordering(lit: &crate::parser::Literal) -> bool {
        matches!(
            lit,
            crate::parser::Literal::Integer(_) | crate::parser::Literal::Float(_)
        )
    }
}

impl PhysicalOperatorExec for IndexScanExec {
    fn open(&mut self, storage: Storage) -> OQLResult<()> {
        self.metrics = IndexScanMetrics::default();
        self.next_cursor = None;
        self.access_path = None;
        self.native_scan = None;
        self.documents.clear();
        self.storage = Some(storage.clone());

        if self.index_manager.is_none() && self.initialize_native_index()? {
            return Ok(());
        }

        if self.direction == ScalarIndexDirection::Descending {
            return Err(OQLError::generic(
                "descending scans require a native typed scalar index",
            ));
        }

        if self.cursor.is_some() {
            return Err(OQLError::generic(
                "keyset cursors require a native typed scalar index",
            ));
        }

        if self.index_manager.is_none() {
            if let Ok(catalog_path) = std::env::var("OWL_INDEX_CATALOG") {
                let loaded = IndexManager::load_catalog_with_backend(
                    catalog_path,
                    Arc::new(RealClock::new()),
                    IndexBackendKind::Rust,
                )
                .map_err(|e| OQLError::generic(&format!("Failed loading index catalog: {}", e)))?;
                self.index_manager = Some(Arc::new(loaded));
            } else {
                if self.index_manager.is_none() {
                    let scalar_defs = storage.list_scalar_indexes(Some(&self.schema))?;
                    if scalar_defs.is_empty() {
                        // Fallback only if no injected manager, no catalog configured,
                        // and the storage backend has no scalar index metadata.
                        self.index_manager = Some(Arc::new(IndexManager::new()));
                    } else {
                        let loaded = IndexManager::load_scalar_definitions_with_backend(
                            scalar_defs,
                            Arc::new(RealClock::new()),
                            IndexBackendKind::Rust,
                        )
                        .map_err(|e| {
                            OQLError::generic(&format!(
                                "Failed loading scalar indexes from storage metadata: {}",
                                e
                            ))
                        })?;
                        loaded.rebuild_from_store(storage.as_ref()).map_err(|e| {
                            OQLError::generic(&format!(
                                "Failed rebuilding scalar indexes from storage: {}",
                                e
                            ))
                        })?;
                        self.index_manager = Some(Arc::new(loaded));
                    }
                }
            }
        }

        // Load documents using index
        self.load_documents_from_index()?;

        Ok(())
    }

    fn next(&mut self) -> OQLResult<Option<Document>> {
        if self.native_scan.is_some() {
            return Ok(self
                .next_batch(1)?
                .and_then(|chunk| chunk.into_documents().into_iter().next()));
        }
        Ok(self.documents.pop_front())
    }

    fn next_batch(&mut self, target_rows: usize) -> OQLResult<Option<DataChunk>> {
        if let Some(mut state) = self.native_scan.take() {
            let result = self.next_native_batch(&mut state, target_rows);
            self.native_scan = Some(state);
            return result;
        }
        if self.documents.is_empty() {
            return Ok(None);
        }
        let take = self.documents.len().min(target_rows.max(1));
        let documents = self.documents.drain(..take).collect();
        Ok(Some(DataChunk::new(documents)))
    }

    fn close(&mut self) -> OQLResult<()> {
        self.documents.clear();
        self.native_scan = None;
        self.storage = None;
        self.index_manager = None;
        Ok(())
    }

    fn reset(&mut self) -> OQLResult<()> {
        let storage = self
            .storage
            .clone()
            .ok_or_else(|| OQLError::generic("Storage not initialized"))?;
        self.open(storage)
    }

    fn runtime_metrics(&self) -> std::collections::BTreeMap<String, u64> {
        let mut metrics = std::collections::BTreeMap::from([
            (
                "index_pages_requested".to_string(),
                self.metrics.index_pages_requested,
            ),
            (
                "index_entries_fetched".to_string(),
                self.metrics.index_entries_fetched,
            ),
            (
                "index_entries_examined".to_string(),
                self.metrics.index_entries_examined,
            ),
            ("records_decoded".to_string(), self.metrics.records_decoded),
            ("rejected_by_key".to_string(), self.metrics.rejected_by_key),
            (
                "rejected_by_prefilter".to_string(),
                self.metrics.rejected_by_prefilter,
            ),
            (
                "rejected_by_residual".to_string(),
                self.metrics.rejected_by_residual,
            ),
        ]);
        if self.prefilter_index.is_some() {
            metrics.insert(
                "prefilter_candidates".to_string(),
                self.metrics.prefilter_candidates,
            );
        }
        metrics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::operator::PhysicalOperatorCollect;
    use crate::parser::{ComparisonOperator, Expression, FieldAccess, Literal};
    use crate::simulation::RealClock;
    use crate::storage::DocumentStore;
    use crate::storage::RustStore;
    use crate::Value;
    use std::collections::HashMap;
    use tempfile::TempDir;

    #[test]
    fn test_extract_key_range_equality() {
        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["id".to_string()],
            }),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::Integer(42)),
        };

        let scan = IndexScanExec::new(
            "users".to_string(),
            "idx_users_id".to_string(),
            vec![condition],
        );

        let (start, end) = scan.extract_key_range().unwrap();
        assert_eq!(start, 42);
        assert_eq!(end, 42);
    }

    #[test]
    fn test_extract_key_range_less_than() {
        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["age".to_string()],
            }),
            operator: ComparisonOperator::LessThan,
            right: Expression::Literal(Literal::Integer(30)),
        };

        let scan = IndexScanExec::new(
            "users".to_string(),
            "idx_users_age".to_string(),
            vec![condition],
        );

        let (start, end) = scan.extract_key_range().unwrap();
        assert_eq!(start, 0);
        assert_eq!(end, 29);
    }

    #[test]
    fn test_extract_key_range_greater_equal() {
        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["score".to_string()],
            }),
            operator: ComparisonOperator::GreaterEqual,
            right: Expression::Literal(Literal::Integer(100)),
        };

        let scan = IndexScanExec::new(
            "users".to_string(),
            "idx_users_score".to_string(),
            vec![condition],
        );

        let (start, end) = scan.extract_key_range().unwrap();
        assert_eq!(start, 100);
        assert_eq!(end, u64::MAX);
    }

    #[test]
    fn test_extract_key_range_no_conditions() {
        let scan = IndexScanExec::new("users".to_string(), "idx_users_id".to_string(), vec![]);

        let (start, end) = scan.extract_key_range().unwrap();
        assert_eq!(start, 0);
        assert_eq!(end, u64::MAX);
    }

    #[test]
    fn test_index_scan_uses_injected_index_manager_documents_match_objref() {
        let temp = TempDir::new().expect("temp dir");
        let db_path = temp.path().join("index_scan_exec.owl");
        let store = Arc::new(
            RustStore::new(db_path.to_str().expect("path"), 10 * 1024 * 1024).expect("store"),
        );

        // Insert docs
        let mut d1 = HashMap::new();
        d1.insert("age".to_string(), Value::Integer(30));
        let mut d2 = HashMap::new();
        d2.insert("age".to_string(), Value::Integer(40));
        store
            .put_document(&Document::with_id(
                "u1".to_string(),
                "users".to_string(),
                d1,
            ))
            .expect("put u1");
        store
            .put_document(&Document::with_id(
                "u2".to_string(),
                "users".to_string(),
                d2,
            ))
            .expect("put u2");

        // Build index manager from same store ObjRefs.
        let manager = Arc::new(IndexManager::new_with_backend(
            Arc::new(RealClock::new()),
            IndexBackendKind::Rust,
        ));
        manager
            .create_index("users", vec!["age".to_string()], false)
            .expect("create index");

        let obj1 = store
            .get_objref("users", &"u1".to_string())
            .expect("obj1")
            .expect("objref1");
        let obj2 = store
            .get_objref("users", &"u2".to_string())
            .expect("obj2")
            .expect("objref2");
        let mut fv1 = HashMap::new();
        fv1.insert("age".to_string(), 30u64);
        let mut fv2 = HashMap::new();
        fv2.insert("age".to_string(), 40u64);
        manager
            .on_insert("users", crate::document::ObjRef::new(obj1.offset()), &fv1)
            .expect("index u1");
        manager
            .on_insert("users", crate::document::ObjRef::new(obj2.offset()), &fv2)
            .expect("index u2");

        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["age".to_string()],
            }),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::Integer(30)),
        };
        let mut scan = IndexScanExec::new_with_index_manager(
            "users".to_string(),
            "idx_users_age".to_string(),
            vec![condition],
            manager,
        );
        scan.open(store.clone()).expect("open scan");
        let docs = scan.collect_all().expect("collect docs");
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].id, "u1".to_string());
    }

    #[test]
    fn test_extract_key_range_string_equality() {
        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["profile".to_string(), "country".to_string()],
            }),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::String("NG".to_string())),
        };

        let scan = IndexScanExec::new(
            "users".to_string(),
            "idx_users_profile_country".to_string(),
            vec![condition],
        );

        let (start, end) = scan.extract_key_range().unwrap();
        assert_eq!(start, end);
        assert_eq!(start, wyhash("NG".as_bytes(), 0));
    }

    #[test]
    fn test_index_scan_supports_nested_path_string_equality() {
        let temp = TempDir::new().expect("temp dir");
        let db_path = temp.path().join("index_scan_nested_path.owl");
        let store = Arc::new(
            RustStore::new(db_path.to_str().expect("path"), 10 * 1024 * 1024).expect("store"),
        );

        let mut profile1 = HashMap::new();
        profile1.insert("country".to_string(), Value::String("NG".to_string()));
        let mut d1 = HashMap::new();
        d1.insert("profile".to_string(), Value::Object(profile1));

        let mut profile2 = HashMap::new();
        profile2.insert("country".to_string(), Value::String("US".to_string()));
        let mut d2 = HashMap::new();
        d2.insert("profile".to_string(), Value::Object(profile2));

        store
            .put_document(&Document::with_id(
                "u1".to_string(),
                "users".to_string(),
                d1,
            ))
            .expect("put u1");
        store
            .put_document(&Document::with_id(
                "u2".to_string(),
                "users".to_string(),
                d2,
            ))
            .expect("put u2");

        let manager = Arc::new(IndexManager::new_with_backend(
            Arc::new(RealClock::new()),
            IndexBackendKind::InMemory,
        ));
        manager
            .create_index("users", vec!["profile.country".to_string()], false)
            .expect("create nested index");

        let obj1 = store
            .get_objref("users", &"u1".to_string())
            .expect("obj1")
            .expect("objref1");
        let obj2 = store
            .get_objref("users", &"u2".to_string())
            .expect("obj2")
            .expect("objref2");

        let mut fv1 = HashMap::new();
        fv1.insert("profile.country".to_string(), wyhash("NG".as_bytes(), 0));
        let mut fv2 = HashMap::new();
        fv2.insert("profile.country".to_string(), wyhash("US".as_bytes(), 0));

        manager
            .on_insert("users", crate::document::ObjRef::new(obj1.offset()), &fv1)
            .expect("index u1");
        manager
            .on_insert("users", crate::document::ObjRef::new(obj2.offset()), &fv2)
            .expect("index u2");

        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["profile".to_string(), "country".to_string()],
            }),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::String("NG".to_string())),
        };

        let mut scan = IndexScanExec::new_with_index_manager(
            "users".to_string(),
            "idx_users_profile.country".to_string(),
            vec![condition],
            manager,
        );
        scan.open(store.clone()).expect("open scan");
        let docs = scan.collect_all().expect("collect docs");
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].id, "u1");
    }

    #[test]
    fn test_index_scan_bootstraps_from_storage_scalar_index_metadata() {
        let temp = TempDir::new().expect("temp dir");
        let db_path = temp.path().join("index_scan_store_metadata.owl");
        let store = Arc::new(
            RustStore::new(db_path.to_str().expect("path"), 10 * 1024 * 1024).expect("store"),
        );

        store
            .upsert_scalar_index(crate::storage::ScalarIndexDefinition {
                name: "idx_users_profile.country".to_string(),
                schema: "users".to_string(),
                fields: vec!["profile.country".to_string()],
                unique: false,
            })
            .expect("persist scalar index metadata");

        let mut profile1 = HashMap::new();
        profile1.insert("country".to_string(), Value::String("NG".to_string()));
        let mut d1 = HashMap::new();
        d1.insert("profile".to_string(), Value::Object(profile1));

        let mut profile2 = HashMap::new();
        profile2.insert("country".to_string(), Value::String("US".to_string()));
        let mut d2 = HashMap::new();
        d2.insert("profile".to_string(), Value::Object(profile2));

        store
            .put_document(&Document::with_id(
                "u1".to_string(),
                "users".to_string(),
                d1,
            ))
            .expect("put u1");
        store
            .put_document(&Document::with_id(
                "u2".to_string(),
                "users".to_string(),
                d2,
            ))
            .expect("put u2");

        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["profile".to_string(), "country".to_string()],
            }),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::String("NG".to_string())),
        };

        let mut scan = IndexScanExec::new(
            "users".to_string(),
            "idx_users_profile.country".to_string(),
            vec![condition],
        );
        scan.open(store.clone()).expect("open scan");
        let docs = scan.collect_all().expect("collect docs");
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].id, "u1");
    }

    #[test]
    fn native_index_scan_preserves_signed_exclusive_range_order() {
        let temp = TempDir::new().expect("temp dir");
        let store = Arc::new(
            RustStore::new(temp.path().join("signed.owl"), 10 * 1024 * 1024).expect("store"),
        );
        store
            .upsert_scalar_index(crate::storage::ScalarIndexDefinition {
                name: "idx_scores_score".into(),
                schema: "scores".into(),
                fields: vec!["score".into()],
                unique: false,
            })
            .unwrap();
        for (id, score) in [("low", -5), ("zero", 0), ("high", 10)] {
            store
                .put_document(&Document::with_id(
                    id.into(),
                    "scores".into(),
                    HashMap::from([("score".into(), Value::Integer(score))]),
                ))
                .unwrap();
        }
        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["score".into()],
            }),
            operator: ComparisonOperator::GreaterThan,
            right: Expression::Literal(Literal::Integer(-1)),
        };
        let mut scan =
            IndexScanExec::new("scores".into(), "idx_scores_score".into(), vec![condition]);
        scan.open(store).unwrap();
        let ids = scan
            .collect_all()
            .unwrap()
            .into_iter()
            .map(|document| document.id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            ids,
            std::collections::HashSet::from(["zero".into(), "high".into()])
        );
    }

    #[test]
    fn native_index_scan_returns_every_non_unique_string_posting() {
        let temp = TempDir::new().expect("temp dir");
        let store = Arc::new(
            RustStore::new(temp.path().join("postings.owl"), 10 * 1024 * 1024).expect("store"),
        );
        store
            .upsert_scalar_index(crate::storage::ScalarIndexDefinition {
                name: "idx_events_level".into(),
                schema: "events".into(),
                fields: vec!["level".into()],
                unique: false,
            })
            .unwrap();
        for (id, level) in [("one", "error"), ("two", "error"), ("three", "warn")] {
            store
                .put_document(&Document::with_id(
                    id.into(),
                    "events".into(),
                    HashMap::from([("level".into(), Value::String(level.into()))]),
                ))
                .unwrap();
        }
        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["level".into()],
            }),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::String("error".into())),
        };
        let mut scan =
            IndexScanExec::new("events".into(), "idx_events_level".into(), vec![condition]);
        scan.open(store).unwrap();
        let ids = scan
            .collect_all()
            .unwrap()
            .into_iter()
            .map(|document| document.id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            ids,
            std::collections::HashSet::from(["one".into(), "two".into()])
        );
    }

    #[test]
    fn native_index_open_is_lazy_and_chunks_decode_only_the_row_goal() {
        let temp = TempDir::new().expect("temp dir");
        let store = Arc::new(
            RustStore::new(temp.path().join("lazy-index.owl"), 10 * 1024 * 1024).expect("store"),
        );
        store
            .upsert_scalar_index(crate::storage::ScalarIndexDefinition {
                name: "idx_events_level".into(),
                schema: "events".into(),
                fields: vec!["level".into()],
                unique: false,
            })
            .unwrap();
        for sequence in 0..5 {
            store
                .put_document(&Document::with_id(
                    format!("event-{sequence}"),
                    "events".into(),
                    HashMap::from([
                        ("level".into(), Value::String("error".into())),
                        ("sequence".into(), Value::Integer(sequence)),
                    ]),
                ))
                .unwrap();
        }
        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["level".into()],
            }),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::String("error".into())),
        };
        let mut scan =
            IndexScanExec::new("events".into(), "idx_events_level".into(), vec![condition])
                .with_limit(3);

        scan.open(store).unwrap();
        assert_eq!(scan.metrics.index_pages_requested, 0);
        assert_eq!(scan.metrics.records_decoded, 0);
        assert_eq!(scan.access_path().unwrap().rows_loaded, 0);

        let first = scan.next_batch(2).unwrap().unwrap();
        assert_eq!(first.len(), 2);
        assert_eq!(scan.metrics.records_decoded, 2);
        assert_eq!(scan.access_path().unwrap().rows_loaded, 2);

        let second = scan.next_batch(2).unwrap().unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(scan.metrics.records_decoded, 3);
        assert_eq!(scan.access_path().unwrap().rows_loaded, 3);
        assert!(scan.next_cursor().is_some());
        assert!(scan.next_batch(2).unwrap().is_none());
    }

    #[test]
    fn incremental_scan_crosses_pages_until_sparse_residual_row_goal() {
        let temp = TempDir::new().expect("temp dir");
        let store = Arc::new(
            RustStore::new(temp.path().join("sparse-residual.owl"), 32 * 1024 * 1024)
                .expect("store"),
        );
        store
            .upsert_scalar_index(crate::storage::ScalarIndexDefinition {
                name: "idx_events_sequence".into(),
                schema: "events".into(),
                fields: vec!["sequence".into()],
                unique: false,
            })
            .unwrap();
        for sequence in 0..300 {
            store
                .put_document(&Document::with_id(
                    format!("event-{sequence}"),
                    "events".into(),
                    HashMap::from([
                        ("sequence".into(), Value::Integer(sequence)),
                        ("keep".into(), Value::Boolean(sequence % 70 == 0)),
                    ]),
                ))
                .unwrap();
        }
        let mut scan = IndexScanExec::new(
            "events".into(),
            "idx_events_sequence".into(),
            vec![FilterCondition::And(
                Box::new(FilterCondition::Comparison {
                    left: Expression::FieldAccess(FieldAccess {
                        path: vec!["sequence".into()],
                    }),
                    operator: ComparisonOperator::GreaterEqual,
                    right: Expression::Literal(Literal::Integer(0)),
                }),
                Box::new(FilterCondition::Comparison {
                    left: Expression::FieldAccess(FieldAccess {
                        path: vec!["keep".into()],
                    }),
                    operator: ComparisonOperator::Equal,
                    right: Expression::Literal(Literal::Boolean(true)),
                }),
            )],
        )
        .with_limit(3);

        scan.open(store).unwrap();
        let documents = scan.collect_all().unwrap();
        let sequences = documents
            .iter()
            .map(|document| document.fields["sequence"].clone())
            .collect::<Vec<_>>();
        assert_eq!(
            sequences,
            vec![Value::Integer(0), Value::Integer(70), Value::Integer(140)],
            "pages={} fetched={} examined={} decoded={} rejected={}",
            scan.metrics.index_pages_requested,
            scan.metrics.index_entries_fetched,
            scan.metrics.index_entries_examined,
            scan.metrics.records_decoded,
            scan.metrics.rejected_by_residual,
        );
        assert!(scan.metrics.index_pages_requested >= 3);
        assert_eq!(scan.metrics.records_decoded, 141);
        assert_eq!(scan.metrics.rejected_by_residual, 138);
        assert!(scan.next_cursor().is_some());
    }

    #[test]
    fn native_index_scan_supports_exact_composite_keys() {
        let temp = TempDir::new().expect("temp dir");
        let store = Arc::new(
            RustStore::new(temp.path().join("composite.owl"), 10 * 1024 * 1024).expect("store"),
        );
        store
            .upsert_scalar_index(crate::storage::ScalarIndexDefinition {
                name: "idx_events_app_level".into(),
                schema: "events".into(),
                fields: vec!["app".into(), "level".into()],
                unique: false,
            })
            .unwrap();
        for (id, app, level) in [
            ("match", "api", "error"),
            ("wrong-level", "api", "warn"),
            ("wrong-app", "web", "error"),
        ] {
            store
                .put_document(&Document::with_id(
                    id.into(),
                    "events".into(),
                    HashMap::from([
                        ("app".into(), Value::String(app.into())),
                        ("level".into(), Value::String(level.into())),
                    ]),
                ))
                .unwrap();
        }
        let equals = |field: &str, value: &str| FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec![field.into()],
            }),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::String(value.into())),
        };
        let mut scan = IndexScanExec::new(
            "events".into(),
            "idx_events_app_level".into(),
            vec![equals("app", "api"), equals("level", "error")],
        );
        scan.open(store).unwrap();
        let documents = scan.collect_all().unwrap();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].id, "match");
    }

    #[test]
    fn native_composite_scan_supports_prefix_and_middle_field_ranges() {
        let temp = TempDir::new().expect("temp dir");
        let store = Arc::new(
            RustStore::new(temp.path().join("composite-range.owl"), 10 * 1024 * 1024)
                .expect("store"),
        );
        store
            .upsert_scalar_index(crate::storage::ScalarIndexDefinition {
                name: "idx_events_app_seq_id".into(),
                schema: "events".into(),
                fields: vec!["app".into(), "seq".into(), "id".into()],
                unique: false,
            })
            .unwrap();
        for (id, app, sequence) in [
            ("five", "api", 5),
            ("six", "api", 6),
            ("ten", "api", 10),
            ("eleven", "api", 11),
            ("other", "web", 7),
        ] {
            store
                .put_document(&Document::with_id(
                    id.into(),
                    "events".into(),
                    HashMap::from([
                        ("app".into(), Value::String(app.into())),
                        ("seq".into(), Value::Integer(sequence)),
                    ]),
                ))
                .unwrap();
        }
        let comparison = |field: &str, operator, literal| FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec![field.into()],
            }),
            operator,
            right: Expression::Literal(literal),
        };
        let mut prefix_scan = IndexScanExec::new(
            "events".into(),
            "idx_events_app_seq_id".into(),
            vec![comparison(
                "app",
                ComparisonOperator::Equal,
                Literal::String("api".into()),
            )],
        );
        prefix_scan.open(store.clone()).unwrap();
        assert_eq!(prefix_scan.collect_all().unwrap().len(), 4);

        let mut range_scan = IndexScanExec::new(
            "events".into(),
            "idx_events_app_seq_id".into(),
            vec![
                comparison(
                    "app",
                    ComparisonOperator::Equal,
                    Literal::String("api".into()),
                ),
                comparison("seq", ComparisonOperator::GreaterThan, Literal::Integer(5)),
                comparison("seq", ComparisonOperator::LessEqual, Literal::Integer(10)),
            ],
        );
        range_scan.open(store).unwrap();
        let ids = range_scan
            .collect_all()
            .unwrap()
            .into_iter()
            .map(|document| document.id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            ids,
            std::collections::HashSet::from(["six".into(), "ten".into()])
        );
    }

    #[test]
    fn native_scan_pushes_limit_and_projection_into_storage() {
        let temp = TempDir::new().expect("temp dir");
        let store = Arc::new(
            RustStore::new(temp.path().join("pushdown.owl"), 10 * 1024 * 1024).expect("store"),
        );
        store
            .upsert_scalar_index(crate::storage::ScalarIndexDefinition {
                name: "idx_events_level".into(),
                schema: "events".into(),
                fields: vec!["level".into()],
                unique: false,
            })
            .unwrap();
        for number in 0..10 {
            store
                .put_document(&Document::with_id(
                    number.to_string(),
                    "events".into(),
                    HashMap::from([
                        ("level".into(), Value::String("error".into())),
                        ("large_payload".into(), Value::String("x".repeat(1024))),
                    ]),
                ))
                .unwrap();
        }
        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["level".into()],
            }),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::String("error".into())),
        };
        let mut scan =
            IndexScanExec::new("events".into(), "idx_events_level".into(), vec![condition])
                .with_limit(2)
                .with_projection(vec!["level".into()]);
        scan.open(store).unwrap();
        let access = scan.access_path().unwrap();
        assert_eq!(access.backend, IndexBackendAccess::NativeTyped);
        assert_eq!(access.rows_loaded, 0);
        assert_eq!(access.projected_fields, vec!["level"]);
        let documents = scan.collect_all().unwrap();
        assert_eq!(documents.len(), 2);
        assert_eq!(scan.access_path().unwrap().rows_loaded, 2);
        assert!(documents
            .iter()
            .all(|document| document.fields.len() == 1 && document.fields.contains_key("level")));
    }

    #[test]
    fn native_keyset_cursor_continues_after_typed_key_and_objref() {
        let temp = TempDir::new().expect("temp dir");
        let store = Arc::new(
            RustStore::new(temp.path().join("cursor.owl"), 10 * 1024 * 1024).expect("store"),
        );
        store
            .upsert_scalar_index(crate::storage::ScalarIndexDefinition {
                name: "idx_events_level".into(),
                schema: "events".into(),
                fields: vec!["level".into()],
                unique: false,
            })
            .unwrap();
        for number in 0..5 {
            store
                .put_document(&Document::with_id(
                    number.to_string(),
                    "events".into(),
                    HashMap::from([("level".into(), Value::String("error".into()))]),
                ))
                .unwrap();
        }
        let condition = FilterCondition::Comparison {
            left: Expression::FieldAccess(FieldAccess {
                path: vec!["level".into()],
            }),
            operator: ComparisonOperator::Equal,
            right: Expression::Literal(Literal::String("error".into())),
        };

        let mut first = IndexScanExec::new(
            "events".into(),
            "idx_events_level".into(),
            vec![condition.clone()],
        )
        .with_limit(2);
        first.open(store.clone()).unwrap();
        let first_ids = first
            .collect_all()
            .unwrap()
            .into_iter()
            .map(|document| document.id)
            .collect::<Vec<_>>();
        let encoded = first.next_cursor().unwrap().encode().unwrap();
        let cursor = ScalarIndexCursor::decode(&encoded).unwrap();

        let mut second =
            IndexScanExec::new("events".into(), "idx_events_level".into(), vec![condition])
                .with_limit(2)
                .with_cursor(cursor);
        second.open(store).unwrap();
        let second_ids = second
            .collect_all()
            .unwrap()
            .into_iter()
            .map(|document| document.id)
            .collect::<Vec<_>>();
        assert_eq!(first_ids.len(), 2);
        assert_eq!(second_ids.len(), 2);
        assert!(first_ids.iter().all(|id| !second_ids.contains(id)));
        assert!(second.next_cursor().is_some());
    }

    #[test]
    fn descending_cursor_pages_reverse_keys_and_duplicate_postings_without_gaps() {
        let temp = TempDir::new().expect("temp dir");
        let store = Arc::new(
            RustStore::new(temp.path().join("descending-cursor.owl"), 10 * 1024 * 1024)
                .expect("store"),
        );
        store
            .upsert_scalar_index(crate::storage::ScalarIndexDefinition {
                name: "idx_events_timestamp".into(),
                schema: "events".into(),
                fields: vec!["timestamp".into()],
                unique: false,
            })
            .unwrap();
        for (id, timestamp) in [
            ("one", 1),
            ("two-a", 2),
            ("two-b", 2),
            ("three", 3),
            ("four", 4),
        ] {
            store
                .put_document(&Document::with_id(
                    id.into(),
                    "events".into(),
                    HashMap::from([("timestamp".into(), Value::Integer(timestamp))]),
                ))
                .unwrap();
        }

        let mut first =
            IndexScanExec::new("events".into(), "idx_events_timestamp".into(), Vec::new())
                .with_direction(ScalarIndexDirection::Descending)
                .with_limit(2);
        first.open(store.clone()).unwrap();
        let mut documents = first.collect_all().unwrap();
        let first_cursor = first.next_cursor().unwrap().clone();
        assert_eq!(first_cursor.direction, ScalarIndexDirection::Descending);

        let mut wrong_direction =
            IndexScanExec::new("events".into(), "idx_events_timestamp".into(), Vec::new())
                .with_cursor(first_cursor.clone());
        assert!(wrong_direction.open(store.clone()).is_err());

        let mut second =
            IndexScanExec::new("events".into(), "idx_events_timestamp".into(), Vec::new())
                .with_direction(ScalarIndexDirection::Descending)
                .with_cursor(first_cursor)
                .with_limit(2);
        second.open(store.clone()).unwrap();
        documents.extend(second.collect_all().unwrap());
        let second_cursor = second.next_cursor().unwrap().clone();

        let mut third =
            IndexScanExec::new("events".into(), "idx_events_timestamp".into(), Vec::new())
                .with_direction(ScalarIndexDirection::Descending)
                .with_cursor(second_cursor)
                .with_limit(2);
        third.open(store).unwrap();
        documents.extend(third.collect_all().unwrap());

        let timestamps = documents
            .iter()
            .map(|document| document.get_field("timestamp").cloned().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            timestamps,
            vec![
                Value::Integer(4),
                Value::Integer(3),
                Value::Integer(2),
                Value::Integer(2),
                Value::Integer(1),
            ]
        );
        let ids = documents
            .iter()
            .map(|document| document.id.clone())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), 5);
    }
}
