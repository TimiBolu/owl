
import {
    Monitor,
    Moon,
    Sun,
    Globe,
    Server,
    Type,
    Shield,
    Save,
    RotateCcw
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
    Card,
    CardContent,
    CardDescription,
    CardHeader,
    CardTitle,
} from "@/components/ui/card";
import {
    Select,
    SelectContent,
    SelectItem,

    SelectTrigger,
    SelectValue,
} from "@/components/ui/select";

// Note: We might need to implement Switch if not available, or use a checkbox substitute.
// Checking if Switch exists in previous context or waiting for error. 
// Safest to use a simple styled checkbox or button toggle if unsure, 
// but let's assume standard UI lib patterns or fallback to simple inputs.
// Actually, let's use standard inputs to be safe and avoid missing component errors.


import { useTheme } from "../theme-provider";

export function Settings() {
    const { theme, setTheme } = useTheme();

    return (
        <div className="flex flex-col h-full bg-background">
            <div className="border-b px-6 py-4">
                <h1 className="text-2xl font-bold tracking-tight">Settings</h1>
                <p className="text-muted-foreground">Manage your workspace preferences and connection details.</p>
            </div>

            <div className="flex-1 overflow-auto p-6 space-y-8">
                {/* Appearance Section */}
                <section className="space-y-4">
                    <div className="flex items-center gap-2 text-lg font-semibold border-b pb-2">
                        <Monitor className="h-5 w-5" />
                        <h2>Appearance</h2>
                    </div>
                    <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
                        <Card>
                            <CardHeader>
                                <CardTitle className="text-base">Theme</CardTitle>
                                <CardDescription>Select your preferred interface theme.</CardDescription>
                            </CardHeader>
                            <CardContent>
                                <Select value={theme} onValueChange={(val: any) => setTheme(val)}>
                                    <SelectTrigger>
                                        <SelectValue placeholder="Select theme" />
                                    </SelectTrigger>
                                    <SelectContent>
                                        <SelectItem value="light">
                                            <div className="flex items-center gap-2">
                                                <Sun className="h-4 w-4" /> Light
                                            </div>
                                        </SelectItem>
                                        <SelectItem value="dark">
                                            <div className="flex items-center gap-2">
                                                <Moon className="h-4 w-4" /> Dark
                                            </div>
                                        </SelectItem>
                                        <SelectItem value="system">
                                            <div className="flex items-center gap-2">
                                                <Monitor className="h-4 w-4" /> System
                                            </div>
                                        </SelectItem>
                                    </SelectContent>
                                </Select>
                            </CardContent>
                        </Card>
                        <Card>
                            <CardHeader>
                                <CardTitle className="text-base">Density</CardTitle>
                                <CardDescription>Adjust the interface information density.</CardDescription>
                            </CardHeader>
                            <CardContent>
                                <Select defaultValue="comfortable">
                                    <SelectTrigger>
                                        <SelectValue placeholder="Select density" />
                                    </SelectTrigger>
                                    <SelectContent>
                                        <SelectItem value="compact">Compact</SelectItem>
                                        <SelectItem value="comfortable">Comfortable</SelectItem>
                                        <SelectItem value="spacious">Spacious</SelectItem>
                                    </SelectContent>
                                </Select>
                            </CardContent>
                        </Card>
                    </div>
                </section>

                {/* Connection Section */}
                <section className="space-y-4">
                    <div className="flex items-center gap-2 text-lg font-semibold border-b pb-2">
                        <Server className="h-5 w-5" />
                        <h2>Connection</h2>
                    </div>
                    <Card>
                        <CardContent className="pt-6 grid gap-4 md:grid-cols-2">
                            <div className="space-y-2">
                                <label className="text-sm font-medium">Host Address</label>
                                <div className="flex gap-2">
                                    <Input defaultValue="localhost" className="font-mono" />
                                </div>
                            </div>
                            <div className="space-y-2">
                                <label className="text-sm font-medium">Port</label>
                                <Input defaultValue="8000" className="font-mono" type="number" />
                            </div>
                            <div className="space-y-2 md:col-span-2">
                                <label className="text-sm font-medium">API Token (Optional)</label>
                                <div className="relative">
                                    <Input type="password" placeholder="••••••••••••••••" className="font-mono pr-10" />
                                    <Shield className="absolute right-3 top-2.5 h-4 w-4 text-muted-foreground" />
                                </div>
                                <p className="text-[0.8rem] text-muted-foreground">
                                    Token for authenticated access to protected OwlDB instances.
                                </p>
                            </div>
                        </CardContent>
                    </Card>
                </section>

                {/* Editor Section */}
                <section className="space-y-4">
                    <div className="flex items-center gap-2 text-lg font-semibold border-b pb-2">
                        <Type className="h-5 w-5" />
                        <h2>Editor</h2>
                    </div>
                    <Card>
                        <CardContent className="pt-6 grid gap-6 md:grid-cols-2">
                            <div className="flex items-center justify-between space-x-2">
                                <div className="flex flex-col space-y-1">
                                    <span className="font-medium">Word Wrap</span>
                                    <span className="text-sm text-muted-foreground">Wrap long lines in the query editor.</span>
                                </div>
                                <Button variant="outline" size="sm" className="w-[80px]">On</Button>
                                {/* Using Button as toggle placeholder since Switch might be missing */}
                            </div>
                            <div className="flex items-center justify-between space-x-2">
                                <div className="flex flex-col space-y-1">
                                    <span className="font-medium">Line Numbers</span>
                                    <span className="text-sm text-muted-foreground">Show line numbers in the gutter.</span>
                                </div>
                                <Button variant="outline" size="sm" className="w-[80px]">On</Button>
                            </div>
                            <div className="flex items-center justify-between space-x-2">
                                <div className="flex flex-col space-y-1">
                                    <span className="font-medium">Format on Paste</span>
                                    <span className="text-sm text-muted-foreground">Automatically format JSON and OQL.</span>
                                </div>
                                <Button variant="secondary" size="sm" className="w-[80px]">Off</Button>
                            </div>
                        </CardContent>
                    </Card>
                </section>

                <div className="flex justify-end gap-2 pt-4">
                    <Button variant="outline">
                        <RotateCcw className="mr-2 h-4 w-4" /> Reset Defaults
                    </Button>
                    <Button>
                        <Save className="mr-2 h-4 w-4" /> Save Changes
                    </Button>
                </div>
            </div>
        </div>
    );
}
