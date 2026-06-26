
import { ReactNode } from "react";

export function Sidebar({ children }: { children: ReactNode }) {
    return (
        <aside className="border-r border-[#d8cec2] bg-[#efe7dd]/80 p-3">
            {children}
        </aside>
    )
}
