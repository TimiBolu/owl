
import * as React from "react";
import { Calculator, Calendar, CreditCard, Settings, User, Monitor, Zap, Database, Search } from "lucide-react";
import { CommandDialog, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList, CommandSeparator } from "cmdk";
import { useTheme } from "../theme-provider";

import "./command-menu.css";

interface CommandMenuProps {
    open: boolean;
    setOpen: (open: boolean) => void;
    onNavigate: (view: string) => void;
    onRunQuery?: () => void;
    toggleTheme: () => void;
}

export function CommandMenu({ open, setOpen, onNavigate, onRunQuery, toggleTheme }: CommandMenuProps) {
    React.useEffect(() => {
        const down = (e: KeyboardEvent) => {
            if (e.key === "k" && (e.metaKey || e.ctrlKey)) {
                e.preventDefault();
                setOpen(!open);
            }
        };

        document.addEventListener("keydown", down);
        return () => document.removeEventListener("keydown", down);
    }, [open, setOpen]); // Added open to deps

    return (
        <CommandDialog open={open} onOpenChange={setOpen} label="Global Command Menu">
            <div className="flex items-center border-b px-3" cmdk-input-wrapper="">
                <Search className="mr-2 h-4 w-4 shrink-0 opacity-50" />
                <CommandInput
                    placeholder="Type a command or search..."
                    className="flex h-11 w-full rounded-md bg-transparent py-3 text-sm outline-none placeholder:text-muted-foreground disabled:cursor-not-allowed disabled:opacity-50"
                />
            </div>
            <CommandList className="max-h-[300px] overflow-y-auto overflow-x-hidden">
                <CommandEmpty>No results found.</CommandEmpty>

                <CommandGroup heading="Suggestions">
                    <CommandItem onSelect={() => { onNavigate("explorer"); setOpen(false); }}>
                        <Database className="mr-2 h-4 w-4" />
                        <span>Go to Explorer</span>
                    </CommandItem>
                    <CommandItem onSelect={() => { onNavigate("studio"); setOpen(false); }}>
                        <Zap className="mr-2 h-4 w-4" />
                        <span>Go to Query Studio</span>
                    </CommandItem>
                    <CommandItem onSelect={() => { onNavigate("monitor"); setOpen(false); }}>
                        <Monitor className="mr-2 h-4 w-4" />
                        <span>Go to Monitor</span>
                    </CommandItem>
                </CommandGroup>

                <CommandSeparator />

                <CommandGroup heading="Actions">
                    <CommandItem onSelect={() => { if (onRunQuery) onRunQuery(); setOpen(false); }}>
                        <Calculator className="mr-2 h-4 w-4" />
                        <span>Run Query</span>
                        <span className="ml-auto text-xs text-muted-foreground">⌘Enter</span>
                    </CommandItem>
                    <CommandItem onSelect={() => { toggleTheme(); setOpen(false); }}>
                        <User className="mr-2 h-4 w-4" />
                        <span>Toggle Theme</span>
                        <span className="ml-auto text-xs text-muted-foreground">⌘T</span>
                    </CommandItem>
                </CommandGroup>

                <CommandSeparator />

                <CommandGroup heading="Settings">
                    <CommandItem onSelect={() => { onNavigate("settings"); setOpen(false); }}>
                        <Settings className="mr-2 h-4 w-4" />
                        <span>Preferences</span>
                        <span className="ml-auto text-xs text-muted-foreground">⌘,</span>
                    </CommandItem>
                </CommandGroup>
            </CommandList>
        </CommandDialog>
    );
}
