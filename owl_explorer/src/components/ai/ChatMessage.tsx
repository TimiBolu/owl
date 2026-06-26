

import { cn } from "@/lib/utils";
import { ChatMessage as MessageType, AgentStep } from "@/lib/ai/types";
import { Bot, User, Terminal, Copy, Play, Brain, Wrench, Eye, Microscope } from "lucide-react";
import { Button } from "../ui/button";


interface ChatMessageProps {
    message: MessageType;
    suggestedActions?: {
        label: string;
        action: "run-query" | "copy-code";
        payload: string;
    }[];
    onAction?: (action: string, payload: string) => void;
}

export function ChatMessage({ message, suggestedActions, onAction }: ChatMessageProps) {
    const isUser = message.role === "user";

    return (
        <div className={cn("flex w-full gap-3 p-4", isUser ? "flex-row-reverse" : "flex-row")}>
            <div className={cn(
                "flex h-8 w-8 shrink-0 items-center justify-center rounded-full border",
                isUser ? "bg-primary text-primary-foreground" : "bg-muted text-muted-foreground"
            )}>
                {isUser ? <User className="h-4 w-4" /> : <Bot className="h-4 w-4" />}
            </div>

            <div className={cn("flex flex-col gap-2 max-w-[80%]", isUser ? "items-end" : "items-start")}>

                {/* Agent Steps (Thinking Process) */}
                {!isUser && message.steps && message.steps.length > 0 && (
                    <div className="flex flex-col gap-1 mb-2 w-full">
                        {message.steps.map(step => (
                            <div key={step.id} className="flex items-center gap-2 text-xs text-muted-foreground/70 animate-in slide-in-from-left-2 fade-in duration-300">
                                {step.type === 'plan' && <Brain className="h-3 w-3 text-purple-400" />}
                                {step.type === 'tool' && <Wrench className="h-3 w-3 text-orange-400" />}
                                {step.type === 'observation' && <Eye className="h-3 w-3 text-blue-400" />}
                                {step.type === 'analysis' && <Microscope className="h-3 w-3 text-green-400" />}
                                <span className={cn(step.type === 'tool' && "font-mono")}>{step.content}</span>
                            </div>
                        ))}
                    </div>
                )}

                <div className={cn(
                    "rounded-lg p-3 text-sm",
                    isUser ? "bg-primary text-primary-foreground" : "bg-muted"
                )}>
                    {message.content}
                </div>


                {/* Suggestions / Actions for Assistant Messages */}
                {!isUser && suggestedActions && suggestedActions.map((action, i) => (
                    <div key={i} className="flex flex-col gap-2 rounded-md border bg-card p-3 shadow-sm w-full">
                        <div className="flex items-center gap-2 text-xs font-medium text-muted-foreground">
                            <Terminal className="h-3 w-3" />
                            <span>Suggested Query</span>
                        </div>
                        <pre className="overflow-x-auto rounded bg-muted/50 p-2 text-xs">
                            <code>{action.payload}</code>
                        </pre>
                        <div className="flex gap-2">
                            <Button
                                size="sm"
                                variant="outline"
                                className="h-7 text-xs"
                                onClick={() => onAction?.(action.action, action.payload)}
                            >
                                <Play className="mr-1 h-3 w-3" />
                                {action.label}
                            </Button>
                            <Button
                                size="sm"
                                variant="ghost"
                                className="h-7 text-xs"
                                onClick={() => {
                                    navigator.clipboard.writeText(action.payload);
                                }}
                            >
                                <Copy className="mr-1 h-3 w-3" />
                                Copy
                            </Button>
                        </div>
                    </div>
                ))}
            </div>
        </div>
    );
}
