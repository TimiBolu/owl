

import { useState, useEffect } from "react";


import { useDatabases } from "./hooks/useDatabases";
import { useSchemas } from "./hooks/useSchemas";
import { useDocuments } from "./hooks/useDocuments";
import { useQuery } from "./hooks/useQuery";
import { useWorkspace } from "./hooks/useWorkspace";
import { useCollections } from "./hooks/useCollections";
import { useDocumentEditing } from "./hooks/useDocumentEditing";
import { useImportExport } from "./hooks/useImportExport";
import { useClusterTopology } from "./hooks/useClusterTopology";

import { Navigator } from "./components/layout/Navigator";
import { WorkspaceTabs } from "./components/workspace/WorkspaceTabs";
import { DatabaseContextView } from "./components/workspace/DatabaseContextView";
import { CollectionView } from "./components/workspace/CollectionView";

import { Button } from "./components/ui/button";

// UI 3.0 Logic

import { Shell } from "./components/layout/Shell";
import { ActivityMode } from "./components/layout/types";



import { Dashboard } from "./components/views/Dashboard";
import { Settings } from "./components/views/Settings";
import { QueryStudio } from "./components/views/QueryStudio";
import { WarRoomView } from "./components/views/WarRoomView";

import { CommandMenu } from "./components/layout/CommandMenu";


function App() {
  const [status, setStatus] = useState("Ready");
  // const [sidebarCollapsed, setSidebarCollapsed] = useState(false); // Handled by ActivityBar
  const [activeActivity, setActiveActivity] = useState<ActivityMode>("explorer");
  const [commandOpen, setCommandOpen] = useState(false);


  // --- Workspace & Navigation ---
  const {
    activeWorkspaceTabId,
    setActiveWorkspaceTabId,
    workspaceTabs,
    ensureDatabaseTab,
    openDatabaseContext,
    openCollectionContext,
    closeWorkspaceTab,
  } = useWorkspace();

  // --- Feature Hooks ---
  const {
    databases, selectedDb, setSelectedDb, dbFilter, setDbFilter,
    pinnedDbs, togglePin, filteredDatabases, refreshDatabases,
    createDatabaseState, deleteDatabase
  } = useDatabases(setStatus);

  const {
    schemas, selectedSchema, setSelectedSchema,
    schemaDetail, refreshSchemas, refreshSchemaDetail,
  } = useSchemas(selectedDb, setStatus);

  const {
    collectionSummaries, collectionLoading, loadCollections,
    deleteCurrentCollection, upsertCollectionSchema, rebuildIndexes,
    collectionFilter, setCollectionFilter, filteredCollections
  } = useCollections(selectedDb, setStatus);

  const {
    docsPage, docsLoading, page, setPage, pageSize, setPageSize,
    docFilter, setDocFilter, visibleBrowsingDocs, refreshDocuments
  } = useDocuments(selectedDb, selectedSchema, setStatus);

  const {
    query, setQuery, queryError, recentQueries, queryMeta,
    visibleQueryDocs, runQuery, resetToInitialDataset,
    queryFilter, setQueryFilter,
    updateQueryDocument, removeQueryDocument // Added missing destructuring
  } = useQuery(selectedDb, setStatus);

  // useDocumentEditing needs to refresh documents on change
  const onDocumentsChanged = async () => {
    await refreshDocuments();
    await loadCollections(selectedDb); // Update counts
  };

  const {
    editingDocId, editingDocSchema, editValues, setEditValues,
    editJsonDraft, setEditJsonDraft, pendingDelete, setPendingDelete,
    startEditingDocument, cancelEditingDocument, saveEditedDocument,
    duplicateDocument, confirmDeleteAction
  } = useDocumentEditing(selectedDb, setStatus, onDocumentsChanged);

  const {
    importDocumentsWithSchema, seedFeatureDemo
  } = useImportExport(selectedDb, setStatus, async () => {
    await refreshSchemas();
    await loadCollections(selectedDb);
  });
  const { nodes: clusterNodes } = useClusterTopology(selectedDb || undefined);


  // --- Effects ---

  // Listen for custom navigation events (hack for deep linking components)
  useEffect(() => {
    const handleNav = (e: any) => {
      const view = e.detail;
      if (view) setActiveActivity(view);
    };
    window.addEventListener("owl:navigate", handleNav);
    return () => window.removeEventListener("owl:navigate", handleNav);
  }, []);

  // Sync selectedDb/Schema with Active Tab

  useEffect(() => {
    const tab = workspaceTabs.find(t => t.id === activeWorkspaceTabId);
    if (!tab) return;

    if (tab.database !== selectedDb) {
      setSelectedDb(tab.database);
    }

    if (tab.type === "collection" && tab.schema && tab.schema !== selectedSchema) {
      setSelectedSchema(tab.schema);
    } else if (tab.type === "database") {
      setSelectedSchema("");
    }
  }, [activeWorkspaceTabId, workspaceTabs, selectedDb, selectedSchema, setSelectedDb, setSelectedSchema]);

  // Load collections when DB changes
  useEffect(() => {
    if (selectedDb) {
      void loadCollections(selectedDb);
      void refreshSchemas();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedDb]);

  // Load documents when Collection changes
  useEffect(() => {
    if (selectedDb && selectedSchema) {
      void refreshDocuments();
      void refreshSchemaDetail();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedDb, selectedSchema]);

  // --- Handlers ---
  const handleSelectDb = (name: string) => {
    setSelectedDb(name);
    openDatabaseContext(name);
  };

  const handleSelectCollection = (dbName: string, colName: string) => {
    if (selectedDb !== dbName) setSelectedDb(dbName);
    setSelectedSchema(colName);
    openCollectionContext(dbName, colName);
  };

  const activeTab = workspaceTabs.find(t => t.id === activeWorkspaceTabId);

  // --- Sidebar Content Construction ---
  let sidebarContent: React.ReactNode = null;
  let sidebarTitle = "";

  if (activeActivity === "explorer") {
    sidebarTitle = "Explorer";
    sidebarContent = (
      <Navigator
        databases={filteredDatabases}
        selectedDb={selectedDb}
        dbFilter={dbFilter}
        onDbFilterChange={setDbFilter}
        onSelectDb={handleSelectDb}
        pinnedDbs={pinnedDbs}
        onTogglePin={togglePin}
        loadingCollections={collectionLoading}
        collections={filteredCollections}
        collectionFilter={collectionFilter}
        onCollectionFilterChange={setCollectionFilter}
        selectedSchema={selectedSchema}
        onSelectCollection={handleSelectCollection}
        createDbState={createDatabaseState}
        ensureDatabaseTab={ensureDatabaseTab}
      />

    );

  } else if (activeActivity === "war_room") {
    sidebarTitle = "Target Logs";
    sidebarContent = (
      <div className="p-4 space-y-4">
        <div className="text-xs font-semibold text-destructive uppercase tracking-wider">Battle History</div>
        <div className="text-xs text-muted-foreground italic">
          Chronicle of chaos events...
        </div>
      </div>
    );
  } else if (activeActivity === "monitor") {

    sidebarTitle = "Monitor";
    sidebarContent = (
        <div className="p-4 space-y-4">
        <div className="text-xs font-semibold text-muted-foreground uppercase tracking-wider">Cluster Nodes</div>
        <div className="space-y-2">
          {(clusterNodes.length ? clusterNodes : [{ id: "owl-node-local", label: "owl-node-local" } as any]).map((n) => (
            <div key={n.id} className="flex items-center gap-2 text-sm">
              <div className="h-2 w-2 rounded-full bg-green-500" />
              <span>{n.label ?? n.id}</span>
            </div>
          ))}
        </div>
      </div>
    );
  } else if (activeActivity === "studio") {
    sidebarTitle = "Studio";
    sidebarContent = (
      <div className="p-4 space-y-4">
        <div className="text-xs font-semibold text-muted-foreground uppercase tracking-wider">Saved Queries</div>
        <div className="space-y-1">
          {recentQueries.length > 0 ? recentQueries.map((q, i) => (
            <div key={i} className="text-xs truncate p-1 hover:bg-muted rounded cursor-pointer" onClick={() => setQuery(q)}>
              {q}
            </div>
          )) : (
            <div className="text-xs text-muted-foreground italic">No saved queries</div>
          )}
        </div>
      </div>
    );
  } else if (activeActivity === "settings") {
    sidebarTitle = "Settings";
    sidebarContent = (
      <div className="p-4 space-y-1">
        <div className="text-sm font-medium hover:underline cursor-pointer">General</div>
        <div className="text-sm font-medium hover:underline cursor-pointer">Appearance</div>
        <div className="text-sm font-medium hover:underline cursor-pointer">Connection</div>
      </div>
    );
  }



  // --- Main Content Construction ---
  let mainVisual: React.ReactNode;


  if (activeActivity === "settings") {
    mainVisual = <Settings />;
  } else if (activeActivity === "monitor") {
    mainVisual = <Dashboard />;
  } else if (activeActivity === "war_room") {
    mainVisual = <WarRoomView />;
  } else if (activeActivity === "studio") {

    mainVisual = (
      <QueryStudio
        query={query}
        onQueryChange={setQuery}
        onRunQuery={async () => { await runQuery(); }}
        queryError={queryError}
        queryMeta={queryMeta}
        visibleDocs={visibleQueryDocs}
        selectedDb={selectedDb}
        recentQueries={recentQueries}
        onSelectHistory={setQuery}

        onUpdateDocument={async (doc) => updateQueryDocument(doc)}

        onDeleteDocument={async (doc) => removeQueryDocument(doc.schema || "", doc.id || (doc as any)._id || "")}


      />
    );
  } else {
    // Explorer Mode (Tabbed Interface)
    if (!activeTab) {
      mainVisual = <Dashboard />;
    } else {
      mainVisual = (
        <div className="flex flex-col h-full">
          <WorkspaceTabs
            tabs={workspaceTabs}
            activeTabId={activeWorkspaceTabId}
            onSelectTab={setActiveWorkspaceTabId}
            onCloseTab={closeWorkspaceTab}
          />


          <div className="flex-1 overflow-auto p-4">
            {activeTab?.type === "database" && (
              <DatabaseContextView
                dbName={activeTab.database}
                collections={collectionSummaries}
                onRefresh={async () => {
                  await refreshDatabases();
                  await loadCollections(activeTab.database);
                }}
                onSeedDemo={seedFeatureDemo}
                onDeleteDb={async () => {
                  if (confirm(`Delete database ${activeTab.database}?`)) {
                    await deleteDatabase(activeTab.database);
                  }
                }}
                onOpenCollection={handleSelectCollection}
                onCreateCollection={async (name, fields, constraints) => {
                  const success = await upsertCollectionSchema(name, fields, constraints, "replace");
                  if (success) {
                    await loadCollections(activeTab.database);
                    openCollectionContext(activeTab.database, name);
                  }
                }}
                onRebuildIndexes={async () => {
                  await rebuildIndexes("memory");
                }}
                onImport={async (schema, fields, constraints, docs, strategy) => {
                  await importDocumentsWithSchema(schema, fields, constraints, docs, strategy);
                }}
              />
            )}

            {activeTab?.type === "collection" && activeTab.schema && (
              <CollectionView
                dbName={activeTab.database}
                collectionName={activeTab.schema}
                onRefresh={onDocumentsChanged}
                onDeleteCollection={async () => {
                  await deleteCurrentCollection(activeTab.schema!, async () => {
                    await loadCollections(activeTab.database);
                    closeWorkspaceTab(activeTab);
                  });
                }}

                // Docs
                docsPage={docsPage}
                docsLoading={docsLoading}
                page={page}
                onPageChange={setPage}
                pageSize={pageSize}
                onPageSizeChange={setPageSize}
                docFilter={docFilter}
                onDocFilterChange={setDocFilter}
                visibleDocs={visibleBrowsingDocs}

                // Query
                query={query}
                onQueryChange={setQuery}
                onRunQuery={async () => {
                  await runQuery();
                }}
                onResetQuery={resetToInitialDataset}
                queryError={queryError}
                recentQueries={recentQueries}
                queryMeta={queryMeta}
                queryFilter={queryFilter}
                onQueryFilterChange={setQueryFilter}
                visibleQueryDocs={visibleQueryDocs}

                // Schema
                schemaDetail={schemaDetail}
                onSaveSchema={async (fields, constraints) => {
                  await upsertCollectionSchema(activeTab.schema!, fields, constraints, "replace");
                  await refreshSchemaDetail();
                }}

                onStatusChange={setStatus}

                // Editing
                editingDocId={editingDocId}
                editingDocSchema={editingDocSchema}
                startEditingDocument={startEditingDocument}
                editValues={editValues}
                setEditValues={setEditValues}
                editJsonDraft={editJsonDraft}
                setEditJsonDraft={setEditJsonDraft}
                saveEditedDocument={saveEditedDocument}
                cancelEditingDocument={cancelEditingDocument}
                duplicateDocument={duplicateDocument}

                pendingDelete={pendingDelete}
                setPendingDelete={setPendingDelete}
              />
            )}
          </div>
        </div>
      );


    }
  }

  return (

    <>
      <Shell
        status={status}
        activeMode={activeActivity}
        onModeChange={setActiveActivity}
        sidebarTitle={sidebarTitle}

        sidebarContent={sidebarContent}
        mainContent={mainVisual}
      />

      <CommandMenu
        open={commandOpen}
        setOpen={setCommandOpen}

        onNavigate={(view) => setActiveActivity(view as ActivityMode)}

        toggleTheme={() => {
          // We need a way to access theme setter here or pass it down. 
          // For now, let's just use a hack or better yet, move CommandMenu inside Shell or context.
          const root = window.document.documentElement;
          const isDark = root.classList.contains("dark");
          root.classList.remove("light", "dark");
          root.classList.add(isDark ? "light" : "dark");
        }}
        onRunQuery={activeActivity === "studio" ? () => {
          // Hard to easy access runQuery from here without context. 
          // Leaving as no-op or we can lift state up.
          // Given the complexity, we will skip the hotkey for now or rely on the button.
          // Or better: use a custom event.
          window.dispatchEvent(new CustomEvent("owl:run-query"));
        } : undefined}
      />


      {/* Global Modals */}
      {pendingDelete && (
        <div className="fixed bottom-0 left-0 right-0 z-50 border-t border-[#bfae9b] bg-[#f7efe2] px-4 py-3 shadow-[0_-4px_12px_rgba(0,0,0,0.08)]">
          <div className="mx-auto flex max-w-[1200px] flex-wrap items-center justify-between gap-3">
            <div className="text-sm">
              {pendingDelete.kind === "one"
                ? `Delete document '${pendingDelete.id}' from '${pendingDelete.schema}'?`
                : `Delete all ${pendingDelete.count} document(s) from '${pendingDelete.schema}'?`}
            </div>
            <div className="flex items-center gap-2">
              <Button variant="secondary" onClick={() => setPendingDelete(null)}>
                Cancel
              </Button>
              <Button onClick={() => void confirmDeleteAction()}>
                Confirm delete
              </Button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}

export default App;
