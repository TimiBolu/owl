
import { useState, useRef, useEffect } from "react";
import { ChatMessage } from "./ChatMessage";
import { ChatMessage as MessageType } from "@/lib/ai/types";
import { getAIService } from "@/lib/ai/service";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { Send, Sparkles, X, Bot } from "lucide-react";
import { cn } from "@/lib/utils";

interface ChatInterfaceProps {
    open: boolean;
    onClose: () => void;
    onRunQuery: (query: string) => void;
}

export function ChatInterface({ open, onClose, onRunQuery }: ChatInterfaceProps) {
    const [messages, setMessages] = useState<MessageType[]>([
        {
            id: "1",
            role: "assistant",
            content: "Hello! I'm your OwlDB assistant. I can help you investigate data, write queries, or analyze performance.",
            timestamp: Date.now()
        }
    ]);
    const [input, setInput] = useState("");
    const [isLoading, setIsLoading] = useState(false);
    const scrollRef = useRef<HTMLDivElement>(null);

    // Auto-scroll to bottom
    useEffect(() => {
        if (scrollRef.current) {
            scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
        }
    }, [messages, open]);

    const handleSend = async () => {
        if (!input.trim() || isLoading) return;

        const userMsg: MessageType = {
            id: crypto.randomUUID(),
            role: "user",
            content: input,
            timestamp: Date.now()
        };

        setMessages(prev => [...prev, userMsg]);
        setInput("");
        setIsLoading(true);

        // Force scroll
        setTimeout(() => {
            if (scrollRef.current) scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
        }, 10);

        try {
            const ai = getAIService();
            const response = await ai.chat({ messages: [...messages, userMsg] });

            setMessages(prev => [...prev, response.message]);

            if (response.suggestedActions) {
                setMessages(prev => {
                    const last = prev[prev.length - 1];
                    (last as any).suggestedActions = response.suggestedActions;
                    return [...prev];
                });
            }

        } catch (error) {
            setMessages(prev => [...prev, {
                id: crypto.randomUUID(),
                role: "assistant",
                content: "Sorry, I encountered an error connecting to the AI service.",
                timestamp: Date.now()
            }]);
        } finally {
            setIsLoading(false);
        }
    };

    if (!open) return null;

    return (
        <div className="flex h-full w-[400px] flex-col border-l bg-background shadow-xl transition-all duration-300 ease-in-out">
            <div className="flex h-14 items-center justify-between border-b px-4">
                <div className="flex items-center gap-2 font-semibold">
                    <Sparkles className="h-4 w-4 text-purple-500" />
                    <span>Owl Assistant</span>
                </div>
                <Button variant="ghost" size="icon" onClick={onClose}>
                    <X className="h-4 w-4" />
                </Button>
            </div>

            <div className="flex-1 overflow-y-auto p-0" ref={scrollRef}>
                <div className="flex flex-col">
                    {messages.map(msg => (
                        <ChatMessage
                            key={msg.id}
                            message={msg}
                            suggestedActions={(msg as any).suggestedActions}
                            onAction={(action, payload) => {
                                if (action === "run-query") {
                                    onRunQuery(payload);
                                }
                            }}
                        />
                    ))}
                    {isLoading && (
                        <div className="flex w-full gap-3 p-4">
                            <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full border bg-muted text-muted-foreground">
                                <Bot className="h-4 w-4 animate-pulse" />
                            </div>
                            <div className="flex items-center gap-1 pt-2">
                                <div className="h-2 w-2 animate-bounce rounded-full bg-muted-foreground/50 [animation-delay:-0.3s]" />
                                <div className="h-2 w-2 animate-bounce rounded-full bg-muted-foreground/50 [animation-delay:-0.15s]" />
                                <div className="h-2 w-2 animate-bounce rounded-full bg-muted-foreground/50" />
                            </div>
                        </div>
                    )}
                </div>
            </div>

            <div className="border-t p-4">
                <form
                    onSubmit={(e) => {
                        e.preventDefault();
                        handleSend();
                    }}
                    className="flex items-center gap-2"
                >
                    <Input
                        value={input}
                        onChange={e => setInput(e.target.value)}
                        placeholder="Ask anything..."
                        disabled={isLoading}
                        className="flex-1"
                    />
                    <Button type="submit" size="icon" disabled={isLoading || !input.trim()}>
                        <Send className="h-4 w-4" />
                    </Button>
                </form>
            </div>
        </div>
    );
}
