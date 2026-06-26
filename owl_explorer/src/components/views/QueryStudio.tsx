
import { useState } from "react";
import { Play, Save, RotateCcw, Clock, Code, Database, FileText } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { Badge } from "@/components/ui/badge";


import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Input } from "@/components/ui/input";


import { DocumentGrid } from "../DocumentGrid";
import { GraphView } from "./GraphView";
import { ChatInterface } from "../ai/ChatInterface";

import { OwlDocument, QueryResponse } from "../../types";


const QUERY_SNIPPETS = {
    "Select All": "users",
    "Filter by Age": "users.where { .age > 25 }",
    "Project Fields": "users.select { name, email }",
    "Nested Select": "users.select { \n  name, \n  orders: .orders.select { total, status }\n}",
    "Aggregation": "orders.select { \n  total_sales: sum(.total), \n  count: count()\n}",
    "Navigation": "orders.where { .customer.age > 30 }",
};

interface QueryStudioProps {
    query: string;
    onQueryChange: (q: string) => void;
    onRunQuery: () => Promise<void>;
    queryError: string | null;
    queryMeta: QueryResponse["meta"] | undefined;
    visibleDocs: OwlDocument[];
    selectedDb: string;
    // History
    recentQueries: string[];
    onSelectHistory: (q: string) => void;
    // CRUD
    onUpdateDocument?: (doc: OwlDocument) => Promise<void>;
    onDeleteDocument?: (doc: OwlDocument) => Promise<void>;
}

export function QueryStudio({
    query,
    onQueryChange,
    onRunQuery,
    queryError,
    queryMeta,
    visibleDocs,
    selectedDb,
    recentQueries,
    onSelectHistory,
    onUpdateDocument,
    onDeleteDocument,
}: QueryStudioProps) {

    const [activeTab, setActiveTab] = useState("editor");
    const [chatOpen, setChatOpen] = useState(false);

    return (
        <div className="flex h-full flex-row overflow-hidden bg-background">
            <div className="flex flex-1 flex-col overflow-hidden">
                {/* Toolbar */}
                <div className="flex items-center justify-between border-b px-4 py-2 bg-muted/20">
                    <div className="flex items-center gap-2">
                        <Code className="h-4 w-4 text-primary" />
                        <span className="font-semibold text-sm">Query Studio</span>
                        {selectedDb && <Badge variant="outline" className="ml-2 font-mono text-xs">{selectedDb}</Badge>}
                    </div>
                    <div className="flex items-center gap-2">
                        <Button
                            variant={chatOpen ? "secondary" : "ghost"}
                            size="sm"
                            onClick={() => setChatOpen(!chatOpen)}
                            className="h-8 gap-2"
                        >
                            <span className="text-purple-500">✨</span>
                            Ask AI
                        </Button>
                        <div className="h-4 w-px bg-border mx-2" />
                        <Select onValueChange={(val) => onQueryChange(QUERY_SNIPPETS[val as keyof typeof QUERY_SNIPPETS])}>
                            <SelectTrigger className="w-[180px] h-8 text-xs">
                                <SelectValue placeholder="Load Snippet..." />
                            </SelectTrigger>
                            <SelectContent>
                                {Object.keys(QUERY_SNIPPETS).map((key) => (
                                    <SelectItem key={key} value={key} className="text-xs">
                                        {key}
                                    </SelectItem>
                                ))}
                            </SelectContent>
                        </Select>
                        <Button size="sm" variant="ghost" className="h-8 w-8 p-0" title="Save Query">
                            <Save className="h-4 w-4" />
                        </Button>
                    </div>
                </div>

                <div className="flex flex-1 overflow-hidden">
                    {/* Editor Pane (Left/Top) */}
                    <div className="flex flex-1 flex-col border-r w-1/2">
                        <div className="flex-1 p-4 flex flex-col gap-4">
                            <Card className="flex-1 flex flex-col shadow-none border-dashed bg-muted/10">
                                <CardHeader className="py-2 px-4 border-b flex flex-row items-center justify-between space-y-0">
                                    <CardTitle className="text-xs font-mono uppercase text-muted-foreground flex items-center gap-2">
                                        <FileText className="h-3 w-3" />
                                        OQL Editor
                                    </CardTitle>
                                    <div className="flex gap-1">
                                        <Button
                                            size="sm"
                                            className="h-6 text-xs bg-primary hover:bg-primary/90"
                                            onClick={() => void onRunQuery()}
                                            disabled={!query.trim()}
                                        >
                                            <Play className="mr-1 h-3 w-3" />
                                            Run
                                        </Button>
                                        <Button size="sm" variant="ghost" className="h-6 w-6 p-0" onClick={() => onQueryChange("")}>
                                            <RotateCcw className="h-3 w-3" />
                                        </Button>
                                    </div>
                                </CardHeader>
                                <CardContent className="flex-1 p-0">
                                    <Textarea
                                        value={query}
                                        onChange={(e) => onQueryChange(e.target.value)}
                                        className="h-full w-full resize-none border-0 rounded-none bg-transparent font-mono text-sm p-4 focus-visible:ring-0"
                                        placeholder="// Enter OQL query here...
 users.where { .age > 25 }.select { name, email }"
                                        spellCheck={false}
                                    />
                                </CardContent>
                            </Card>

                            {/* Query Status / Meta */}
                            <div className="h-[120px] shrink-0">
                                {queryError ? (
                                    <div className="h-full rounded-md border border-destructive/50 bg-destructive/10 p-4 text-xs font-mono text-destructive overflow-auto">
                                        <h4 className="font-bold mb-1">Error Executing Query:</h4>
                                        {queryError}
                                    </div>
                                ) : queryMeta ? (
                                    <div className="h-full rounded-md border border-green-500/20 bg-green-500/5 p-4 text-xs font-mono space-y-1">
                                        <div className="flex items-center gap-2 text-green-600 font-semibold mb-2">
                                            <div className="h-2 w-2 rounded-full bg-green-500" />
                                            Query Successful
                                        </div>
                                        <div className="grid grid-cols-2 gap-x-8 gap-y-1 text-muted-foreground">
                                            <span>Execution Time:</span>
                                            <span className="text-foreground">{queryMeta.execution_time_ms.toFixed(2)}ms</span>
                                            <span>Documents Scanned:</span>
                                            <span className="text-foreground">{queryMeta.documents_scanned}</span>
                                            <span>Documents Returned:</span>
                                            <span className="text-foreground">{queryMeta.documents_returned}</span>
                                            <span title="Navigation Hops">Nav Hops:</span>
                                            <span className="text-foreground">-</span>
                                        </div>
                                    </div>
                                ) : (
                                    <div className="h-full rounded-md border border-dashed flex items-center justify-center text-xs text-muted-foreground">
                                        Ready to run query
                                    </div>
                                )}
                            </div>
                        </div>
                    </div>

                    {/* Results Pane (Right/Bottom) */}
                    <div className="flex flex-1 flex-col w-1/2 bg-muted/5">
                        <Tabs defaultValue="json" className="flex-1 flex flex-col">
                            <div className="border-b px-4 flex items-center justify-between bg-background">
                                <TabsList className="h-10 bg-transparent p-0">
                                    <TabsTrigger value="table" className="h-10 rounded-none border-b-2 border-transparent px-4 data-[state=active]:border-primary data-[state=active]:bg-transparent">
                                        Table
                                    </TabsTrigger>
                                    <TabsTrigger value="json" className="h-10 rounded-none border-b-2 border-transparent px-4 data-[state=active]:border-primary data-[state=active]:bg-transparent">
                                        JSON
                                    </TabsTrigger>

                                    <TabsTrigger value="graph" className="h-10 rounded-none border-b-2 border-transparent px-4 data-[state=active]:border-primary data-[state=active]:bg-transparent">
                                        Graph Nodes
                                    </TabsTrigger>
                                </TabsList>

                                <span className="text-xs text-muted-foreground">
                                    {visibleDocs.length} Results
                                </span>
                            </div>

                            <TabsContent value="table" className="flex-1 p-0 m-0 overflow-hidden">
                                <div className="h-full overflow-auto p-4">
                                    <DocumentGrid
                                        documents={visibleDocs}
                                        viewMode="grid"
                                        onUpdateDocument={onUpdateDocument}
                                        onDeleteDocument={onDeleteDocument}
                                    />
                                </div>
                            </TabsContent>

                            <TabsContent value="json" className="flex-1 p-0 m-0 overflow-hidden">
                                <div className="h-full overflow-auto">
                                    <div className="p-4 font-mono text-xs whitespace-pre-wrap">
                                        {visibleDocs.length > 0 ? JSON.stringify(visibleDocs, null, 2) : <span className="text-muted-foreground italic">// No results</span>}
                                    </div>
                                </div>
                            </TabsContent>

                            <TabsContent value="graph" className="flex-1 p-0 m-0 overflow-hidden">
                                <div className="h-full w-full bg-background relative">
                                    {visibleDocs.length > 0 ? (
                                        <GraphView documents={visibleDocs} />
                                    ) : (
                                        <div className="flex h-full items-center justify-center text-muted-foreground p-10">
                                            <div className="text-center">
                                                <div className="mb-4 text-4xl opacity-20">🕸️</div>
                                                <p>Run a query to visualize results as a graph.</p>
                                            </div>
                                        </div>
                                    )}
                                </div>
                            </TabsContent>
                        </Tabs>
                    </div>
                </div>
            </div>

            {/* AI Chat Panel */}
            <ChatInterface
                open={chatOpen}
                onClose={() => setChatOpen(false)}
                onRunQuery={(q) => {
                    onQueryChange(q);
                    // void onRunQuery(); // Optional: auto-run
                }}
            />
        </div>
    );
}

