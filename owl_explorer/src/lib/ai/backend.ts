import { api } from "../api";
import { AICompletionRequest, AICompletionResponse, AIService } from "./types";

type ChatEnvelope = { data: AICompletionResponse };
type GenerateEnvelope = { data: { query: string } };
type ExplainEnvelope = { data: { explanation: string } };

export class BackendAIService implements AIService {
    async chat(request: AICompletionRequest): Promise<AICompletionResponse> {
        const res = await api<ChatEnvelope>("/api/ai/chat", { messages: request.messages });
        return res.data;
    }

    async generateQuery(prompt: string): Promise<string> {
        const res = await api<GenerateEnvelope>("/api/ai/generate_query", { prompt });
        return res.data.query;
    }

    async explainQuery(query: string): Promise<string> {
        const res = await api<ExplainEnvelope>("/api/ai/explain_query", { query });
        return res.data.explanation;
    }
}
