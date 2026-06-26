
export interface ChatMessage {
    id: string;
    role: "user" | "assistant" | "system";
    content: string;

    timestamp: number;
    steps?: AgentStep[];
}

export interface AgentStep {
    id: string;
    type: 'plan' | 'tool' | 'observation' | 'analysis';
    content: string;
    timestamp: number;
}


export interface AICompletionRequest {
    messages: ChatMessage[];
    context?: any; // Schema, current query, etc.
}

export interface AICompletionResponse {
    message: ChatMessage;
    suggestedActions?: {
        label: string;
        action: "run-query" | "copy-code";
        payload: string;
    }[];
}

export interface AIService {
    chat(request: AICompletionRequest): Promise<AICompletionResponse>;
    generateQuery(prompt: string, schema: any): Promise<string>;
    explainQuery(query: string): Promise<string>;
}
