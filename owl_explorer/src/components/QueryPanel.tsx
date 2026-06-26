
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { FileCode2, Play } from "lucide-react";

type QueryPanelProps = {
    query: string;
    onQueryChange: (v: string) => void;
    onRun: () => void;
    onReset: () => void;
    error: string;
    recentQueries: string[];
    snippets?: Array<{ label: string; query: string }>;
};

export function QueryPanel({
    query,
    onQueryChange,
    onRun,
    onReset,
    error,
    recentQueries,
    snippets,
}: QueryPanelProps) {
    return (
        <Card>
            <CardHeader>
                <CardTitle className="text-sm tracking-wide">Query</CardTitle>
            </CardHeader>
            <CardContent className="space-y-2">
                <div className="flex flex-wrap gap-2">
                    <Button
                        variant="secondary"
                        size="sm"
                        onClick={() => {
                            const formatted = query
                                .split("\n")
                                .map((line) => line.trim())
                                .join("\n")
                                .trim();
                            onQueryChange(formatted);
                        }}
                    >
                        <FileCode2 className="mr-2 h-4 w-4" />Format
                    </Button>

                    <select
                        className="h-9 rounded-md border border-input bg-card px-3 text-sm"
                        onChange={(e) => e.target.value && onQueryChange(e.target.value)}
                        defaultValue=""
                    >
                        <option value="">History</option>
                        {recentQueries.map((q) => (
                            <option key={q} value={q}>
                                {q.slice(0, 80)}
                            </option>
                        ))}
                    </select>

                    {snippets && (
                        <select
                            className="h-9 rounded-md border border-input bg-card px-3 text-sm"
                            onChange={(e) => e.target.value && onQueryChange(e.target.value)}
                            defaultValue=""
                        >
                            <option value="">Snippets</option>
                            {snippets.map(s => (
                                <option key={s.label} value={s.query}>{s.label}</option>
                            ))}
                        </select>
                    )}
                </div>
                <div className="grid gap-2 md:grid-cols-[1fr_auto_auto]">
                    <Textarea
                        className="mono min-h-[180px]"
                        value={query}
                        onChange={(e) => onQueryChange(e.target.value)}
                        placeholder='orders.where { .status == "paid" }.select { id, amount }'
                        onKeyDown={(e) => {
                            if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
                                e.preventDefault();
                                onRun();
                            }
                        }}
                    />
                    <Button onClick={onRun}>
                        <Play className="mr-2 h-4 w-4" />Run
                    </Button>
                    <Button variant="secondary" onClick={onReset}>Reset</Button>
                </div>
                {error && <div className="rounded-md border border-red-300 bg-red-50 px-3 py-2 text-sm text-red-800">{error}</div>}
            </CardContent>
        </Card>
    );
}
