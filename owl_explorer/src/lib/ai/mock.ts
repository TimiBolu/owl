
import { AICompletionRequest, AICompletionResponse, AIService, ChatMessage, AgentStep } from "./types";
import { OQLParser } from "../oql/parser";

export class MockAIService implements AIService {
    private parser = new OQLParser();

    async chat(request: AICompletionRequest): Promise<AICompletionResponse> {
        const lastUserMessage = request.messages[request.messages.length - 1].content.toLowerCase();
        const steps: AgentStep[] = [];

        // Helper to add steps with delays
        const addStep = async (type: AgentStep['type'], content: string, delay = 500) => {
            await new Promise(resolve => setTimeout(resolve, delay));
            steps.push({
                id: crypto.randomUUID(),
                type,
                content,
                timestamp: Date.now()
            });
        };

        await addStep('plan', 'Analyzing user intent...');

        let content = "I'm not sure how to help with that yet.";
        let suggestedActions = undefined;

        if (lastUserMessage.includes("users") || lastUserMessage.includes("find") || lastUserMessage.includes("search")) {
            await addStep('analysis', 'Identified intent: Data Retrieval');
            await addStep('tool', 'Checking schema for "users" collection...');

            // Extract criteria (naive)
            let query = "";
            if (lastUserMessage.includes("old") || lastUserMessage.includes("age") || lastUserMessage.includes(">")) {
                await addStep('plan', 'Constructing filter for "age" field');
                query = "users.where { .age > 25 }";
            } else if (lastUserMessage.includes("admin")) {
                await addStep('plan', 'Constructing filter for "role" field');
                query = "users.where { .role == 'admin' }";
            } else {
                query = "users";
            }

            await addStep('tool', `Generated OQL Candidate: ${query}`);

            try {
                // Validate with Parser
                await addStep('analysis', 'Validating OQL syntax...');
                this.parser.parse(query); // Will throw if invalid
                await addStep('observation', 'Syntax Validated. Ready to execute.');

                content = `I've constructed a query to find the users you asked for.`;
                suggestedActions = [{
                    label: "Run Query",
                    action: "run-query",
                    payload: query
                }];
            } catch (e) {
                await addStep('analysis', `Syntax Error: ${(e as Error).message}`);
                content = "I tried to generate a query, but the OQL syntax was invalid.";
            }

        } else if (lastUserMessage.includes("hello") || lastUserMessage.includes("hi")) {
            await addStep('observation', 'User is greeting.');
            content = "Hello! I am your Sentient OwlDB assistant. I can help you write queries, explain data, or debug performance locally.";
        } else {
            await addStep('analysis', 'Intent unclear. Offering general help.');
            content = "I can help you query your data using OQL. Try asking 'Find users over 25' or 'Show orders with status pending'.";
        }

        return {
            message: {
                id: crypto.randomUUID(),
                role: "assistant",
                content,
                timestamp: Date.now(),
                steps
            },
            suggestedActions: suggestedActions as any
        };
    }

    async generateQuery(prompt: string, schema: any): Promise<string> {
        await new Promise(resolve => setTimeout(resolve, 1200));
        // Simple mock generation
        if (prompt.includes("users")) {
            return "users.where { .active == true }";
        }
        return "users";
    }

    async explainQuery(query: string): Promise<string> {
        await new Promise(resolve => setTimeout(resolve, 800));
        try {
            const ast = this.parser.parse(query);
            return `This query fetches documents from **${ast.collection}**${ast.where ? ' where conditions are met' : ''}. It is a valid OQL query.`;
        } catch (e) {
            return "Invalid OQL query.";
        }
    }
}

