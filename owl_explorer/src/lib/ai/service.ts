
import { BackendAIService } from "./backend";
import { AIService } from "./types";

// Singleton instance
let instance: AIService | null = null;

export function getAIService(): AIService {
    if (!instance) {
        instance = new BackendAIService();
    }
    return instance;
}
