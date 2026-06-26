
import { Input } from "@/components/ui/input";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";

type SchemaListProps = {
    schemas: string[];
    selectedSchema: string;
    filter: string;
    onFilterChange: (v: string) => void;
    onSelect: (schema: string) => void;
};

export function SchemaList({
    schemas,
    selectedSchema,
    filter,
    onFilterChange,
    onSelect,
}: SchemaListProps) {
    return (
        <Card className="mt-3">
            <CardHeader>
                <CardTitle className="text-sm tracking-wide">Schemas</CardTitle>
            </CardHeader>
            <CardContent className="space-y-2">
                <Input placeholder="Search schemas..." value={filter} onChange={(e) => onFilterChange(e.target.value)} />
                <div className="max-h-56 space-y-1.5 overflow-auto pr-1">
                    {schemas.map((schema) => (
                        <button
                            key={schema}
                            className={`w-full rounded-md border px-2 py-1.5 text-left text-sm ${selectedSchema === schema ? "border-[#7a5a44] bg-[#ebdfd2]" : "border-border bg-card"}`}
                            onClick={() => onSelect(schema)}
                        >
                            {schema}
                        </button>
                    ))}
                </div>
            </CardContent>
        </Card>
    );
}
