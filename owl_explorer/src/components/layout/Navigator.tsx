
import { CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { DatabaseEntry, CollectionSummary } from "../../types";
import { CreateDatabaseForm } from "../CreateDatabaseForm";

type NavigatorProps = {
    // DB Props
    databases: DatabaseEntry[];
    selectedDb: string;
    dbFilter: string;
    onDbFilterChange: (v: string) => void;
    onSelectDb: (name: string) => void;
    pinnedDbs: string[];
    onTogglePin: (name: string) => void;
    // Collection Props
    loadingCollections: boolean;
    collections: CollectionSummary[];
    collectionFilter: string;
    onCollectionFilterChange: (v: string) => void;
    selectedSchema: string;
    onSelectCollection: (dbName: string, colName: string) => void;
    // Create DB Props
    createDbState: {
        dbName: string;
        setDbName: (v: string) => void;
        dbPath: string;
        setDbPath: (v: string) => void;
        dbSize: string;
        setDbSize: (v: string) => void;
        createDatabase: () => Promise<void>;
    };
    ensureDatabaseTab: (db: string) => void;
};

export function Navigator({
    databases,
    selectedDb,
    dbFilter,
    onDbFilterChange,
    onSelectDb,
    pinnedDbs,
    onTogglePin,
    loadingCollections,
    collections,
    collectionFilter,
    onCollectionFilterChange,
    selectedSchema,
    onSelectCollection,
    createDbState,
    ensureDatabaseTab,
}: NavigatorProps) {
    return (
        <div className="flex flex-col h-full bg-muted/10">
            <div className="p-2 space-y-2">
                <Input placeholder="Search databases..." value={dbFilter} onChange={(e) => onDbFilterChange(e.target.value)} className="h-8" />

                <div className="space-y-1">
                    {databases.map((db) => {
                        const isSelectedDb = db.name === selectedDb;
                        return (
                            <div
                                key={db.name}
                                className={`rounded-sm border ${isSelectedDb ? "border-primary/20 bg-primary/5" : "border-transparent hover:bg-muted/50"}`}
                            >
                                <div className="flex items-center justify-between p-2">
                                    <button className="text-left flex-1" onClick={() => onSelectDb(db.name)}>
                                        <div className="font-medium text-sm flex items-center gap-2">
                                            {pinnedDbs.includes(db.name) && <span className="text-xs text-yellow-500">★</span>}
                                            {db.name}
                                        </div>
                                    </button>
                                    <button
                                        className="text-[10px] text-muted-foreground opacity-0 hover:opacity-100 transition-opacity px-1"
                                        onClick={() => onTogglePin(db.name)}
                                    >
                                        PIN
                                    </button>
                                </div>

                                {isSelectedDb && (
                                    <div className="pl-4 pr-2 pb-2 space-y-0.5">
                                        <Input
                                            placeholder="Filter..."
                                            value={collectionFilter}
                                            onChange={(e) => onCollectionFilterChange(e.target.value)}
                                            className="h-6 text-xs mb-2"
                                        />

                                        {loadingCollections ? (
                                            <div className="text-xs text-muted-foreground py-1">Loading...</div>
                                        ) : (
                                            collections.map((collection) => {
                                                const isSelectedCollection = collection.name === selectedSchema;
                                                return (
                                                    <button
                                                        key={collection.name}
                                                        className={`flex w-full items-center justify-between rounded px-2 py-1 text-left text-xs ${isSelectedCollection ? "bg-primary text-primary-foreground" : "hover:bg-muted"
                                                            }`}
                                                        onClick={() => onSelectCollection(db.name, collection.name)}
                                                    >
                                                        <span className="truncate">{collection.name}</span>
                                                        <span className={`text-[10px] ${isSelectedCollection ? "text-primary-foreground/70" : "text-muted-foreground"}`}>
                                                            {collection.document_count}
                                                        </span>
                                                    </button>
                                                );
                                            })
                                        )}
                                        <div className="pt-2">
                                            <CreateDatabaseForm
                                                name={createDbState.dbName}
                                                path={createDbState.dbPath}
                                                size={createDbState.dbSize}
                                                onNameChange={createDbState.setDbName}
                                                onPathChange={createDbState.setDbPath}
                                                onSizeChange={createDbState.setDbSize}
                                                onCreate={async () => {
                                                    await createDbState.createDatabase();
                                                    if (createDbState.dbName.trim()) {
                                                        ensureDatabaseTab(createDbState.dbName.trim());
                                                    }
                                                }}
                                            />
                                        </div>
                                    </div>
                                )}
                            </div>
                        );
                    })}
                </div>
            </div>
        </div>
    );
}

