
import { Input } from "@/components/ui/input";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Search, Star, StarOff } from "lucide-react";
import { DatabaseEntry } from "../types";

type DatabaseListProps = {
    databases: DatabaseEntry[];
    selectedDb: string;
    onSelect: (name: string) => void;
    filter: string;
    onFilterChange: (value: string) => void;
    pinnedDbs: string[];
    onTogglePin: (name: string) => void;
};

export function DatabaseList({
    databases,
    selectedDb,
    onSelect,
    filter,
    onFilterChange,
    pinnedDbs,
    onTogglePin,
}: DatabaseListProps) {
    return (
        <Card>
            <CardHeader>
                <CardTitle className="text-sm tracking-wide">Databases</CardTitle>
            </CardHeader>
            <CardContent className="space-y-2">
                <div className="relative">
                    <Search className="pointer-events-none absolute left-2 top-2.5 h-4 w-4 text-muted-foreground" />
                    <Input className="pl-8" placeholder="Search databases..." value={filter} onChange={(e) => onFilterChange(e.target.value)} />
                </div>
                <div className="max-h-52 space-y-2 overflow-auto pr-1">
                    {databases.map((db) => (
                        <div
                            key={db.name}
                            className={`w-full rounded-md border p-2 text-left ${selectedDb === db.name ? "border-[#7a5a44] bg-[#ebdfd2]" : "border-border bg-card"}`}
                            role="button"
                            tabIndex={0}
                            onClick={() => onSelect(db.name)}
                            onKeyDown={(e) => {
                                if (e.key === "Enter" || e.key === " ") {
                                    e.preventDefault();
                                    onSelect(db.name);
                                }
                            }}
                        >
                            <div className="flex items-center justify-between gap-2">
                                <div className="font-semibold">{db.name}</div>
                                <button
                                    className="rounded border border-border p-1 hover:bg-secondary"
                                    onClick={(e) => {
                                        e.stopPropagation();
                                        onTogglePin(db.name);
                                    }}
                                >
                                    {pinnedDbs.includes(db.name) ? <Star className="h-3.5 w-3.5" /> : <StarOff className="h-3.5 w-3.5" />}
                                </button>
                            </div>
                            <div className="mt-0.5 text-xs text-muted-foreground">{db.path}</div>
                        </div>
                    ))}
                </div>
            </CardContent>
        </Card>
    );
}
