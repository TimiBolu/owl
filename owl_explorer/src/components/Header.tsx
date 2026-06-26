
import { DatabaseZap } from "lucide-react";

type HeaderProps = {
    status: string;
};

export function Header({ status }: HeaderProps) {
    return (
        <header className="sticky top-0 z-40 border-b border-[#d3c8bb] bg-[#111827] text-white">
            <div className="flex h-12 items-center justify-between px-4">
                <div className="flex items-center gap-3">
                    <DatabaseZap className="h-4 w-4 text-[#f59e0b]" />
                    <span className="text-sm font-semibold tracking-wide">OWL EXPLORER</span>
                </div>
                <div className="text-xs text-slate-300">{status}</div>
            </div>
        </header>
    );
}
