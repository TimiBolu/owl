
import { Database } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { SchemaDetail as SchemaDetailType } from "../types";

type SchemaDetailProps = {
    schemaDetail: SchemaDetailType | null;
};

export function SchemaDetail({ schemaDetail }: SchemaDetailProps) {
    return (
        <Card>
            <CardHeader>
                <CardTitle className="text-sm tracking-wide">Schema Detail</CardTitle>
            </CardHeader>
            <CardContent>
                {!schemaDetail ? (
                    <div className="text-sm text-muted-foreground">Select a schema</div>
                ) : (
                    <div className="space-y-2">
                        <div className="flex items-center gap-2">
                            <Database className="h-4 w-4" />
                            <span className="font-semibold">{schemaDetail.name}</span>
                            <span className="text-xs text-muted-foreground">
                                constraints: {schemaDetail.constraints?.length ?? 0}
                            </span>
                        </div>
                        <div className="grid gap-2 md:grid-cols-2">
                            {schemaDetail.fields.map((field) => (
                                <div key={field.name} className="rounded-md border border-[#d6cabd] bg-[#f8f2e8] px-3 py-2 text-sm">
                                    <div className="font-semibold">{field.name}</div>
                                    <div className="text-xs text-muted-foreground">{field.type}</div>
                                </div>
                            ))}
                        </div>
                        {!!schemaDetail.constraints?.length && (
                            <div className="rounded-md border border-[#d6cabd] bg-[#f8f2e8] p-3">
                                <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                                    Constraints
                                </div>
                                <pre className="mono overflow-x-auto text-xs">
                                    {JSON.stringify(schemaDetail.constraints, null, 2)}
                                </pre>
                            </div>
                        )}
                    </div>
                )}
            </CardContent>
        </Card>
    );
}
