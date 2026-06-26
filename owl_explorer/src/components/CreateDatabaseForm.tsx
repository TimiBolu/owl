
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Plus } from "lucide-react";

type CreateDatabaseFormProps = {
    name: string;
    path: string;
    size: string;
    onNameChange: (v: string) => void;
    onPathChange: (v: string) => void;
    onSizeChange: (v: string) => void;
    onCreate: () => void;
};

export function CreateDatabaseForm({
    name,
    path,
    size,
    onNameChange,
    onPathChange,
    onSizeChange,
    onCreate,
}: CreateDatabaseFormProps) {
    return (
        <Card className="mt-3">
            <CardHeader>
                <CardTitle className="text-sm tracking-wide">Create Database</CardTitle>
            </CardHeader>
            <CardContent className="space-y-2">
                <Input placeholder="name" value={name} onChange={(e) => onNameChange(e.target.value)} />
                <Input placeholder="path e.g. data/dev.owl" value={path} onChange={(e) => onPathChange(e.target.value)} />
                <Input type="number" min={1} value={size} onChange={(e) => onSizeChange(e.target.value)} />
                <Button className="w-full" onClick={onCreate}>
                    <Plus className="mr-2 h-4 w-4" />Create
                </Button>
            </CardContent>
        </Card>
    );
}
