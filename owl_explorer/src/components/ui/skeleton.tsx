import { cn } from "@/lib/utils";

function Skeleton({ className }: { className?: string }) {
  return <div className={cn("animate-pulse rounded-md bg-[#eadfd2]", className)} />;
}

export { Skeleton };
