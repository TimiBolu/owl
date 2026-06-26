

import { OQLExpression, OQLQuery } from './types';


/**
 * A simple regex-based parser for OQL (Owl Query Language).
 * Syntax: collection.where { condition }.select { fields }.limit(n)
 */
export class OQLParser {
    parse(input: string): OQLQuery {
        const query: OQLQuery = { collection: '' };

        // Remove comments
        const cleanInput = input.replace(/\/\/.*$/gm, '').replace(/\n/g, ' ').trim();

        // 1. Extract Collection
        const collectionMatch = cleanInput.match(/^([a-zA-Z0-9_]+)/);
        if (!collectionMatch) throw new Error("Invalid OQL: Must start with collection name");
        query.collection = collectionMatch[1];

        let remaining = cleanInput.substring(collectionMatch[0].length);

        // 2. Parse Method Chains
        // This is a naive loop that consumes .method(...) patterns
        while (remaining.length > 0) {
            remaining = remaining.trim();
            if (remaining.length === 0) break;

            if (remaining.startsWith('.')) {
                if (remaining.startsWith('.where')) {
                    const block = this.extractBlock(remaining, 'where');
                    query.where = this.parseExpression(block.content);
                    remaining = block.remaining;
                } else if (remaining.startsWith('.select')) {
                    const block = this.extractBlock(remaining, 'select');
                    query.select = block.content.split(',').map(s => s.trim());
                    remaining = block.remaining;
                } else if (remaining.startsWith('.limit')) {
                    const match = remaining.match(/^\.limit\(\s*(\d+)\s*\)/);
                    if (match) {
                        query.limit = parseInt(match[1]);
                        remaining = remaining.substring(match[0].length);
                    } else {
                        throw new Error("Invalid limit syntax. Expected .limit(n)");
                    }
                } else {
                    throw new Error(`Unknown method in OQL: ${remaining.substring(0, 20)}...`);
                }
            } else {
                throw new Error(`Unexpected syntax: ${remaining.substring(0, 20)}...`);
            }
        }

        return query;
    }

    private extractBlock(input: string, method: string): { content: string, remaining: string } {
        // Matches .method { ... }
        const startRegex = new RegExp(`^\\.${method}\\s*\\{`);
        const match = input.match(startRegex);
        if (!match) throw new Error(`Expected .${method} { ... }`); // Should have been checked before

        let openBraces = 0;
        let contentStart = match[0].length;
        let i = contentStart - 1; // Start at the first {

        // Find matching closing brace
        // We actually need to find the *first* brace in the match to start counting properly if regex consumed it
        // But for simplicity, let's assume standard formatting or do a proper scan

        // Scan from the first '{' found by regex
        const startIndex = input.indexOf('{');
        i = startIndex;
        openBraces = 1;

        for (let j = i + 1; j < input.length; j++) {
            if (input[j] === '{') openBraces++;
            if (input[j] === '}') openBraces--;

            if (openBraces === 0) {
                return {
                    content: input.substring(startIndex + 1, j).trim(),
                    remaining: input.substring(j + 1)
                };
            }
        }

        throw new Error(`Unclosed brace in .${method} block`);
    }

    private parseExpression(expr: string): OQLExpression {
        // Very basic expression parser. 
        // Handles: .field > value, .field == 'value', etc.
        // Returns a flat binary expression for now (no precedence/nesting support in this v1)

        const operatorRegex = /(.+?)\s*(==|!=|>|<|>=|<=)\s*(.+)/;
        const match = expr.match(operatorRegex);

        if (match) {
            const leftRaw = match[1].trim();
            const op = match[2];
            const rightRaw = match[3].trim();

            return {
                type: 'binary',
                operator: op,
                left: this.parseOperand(leftRaw),
                right: this.parseOperand(rightRaw)
            };
        }

        // Fallback for single property (bool)
        return this.parseOperand(expr);
    }

    private parseOperand(raw: string): OQLExpression {
        if (raw.startsWith('.')) {
            return { type: 'property', name: raw.substring(1) };
        }
        if (raw.startsWith("'") || raw.startsWith('"')) {
            return { type: 'literal', value: raw.substring(1, raw.length - 1) };
        }
        if (!isNaN(Number(raw))) {
            return { type: 'literal', value: Number(raw) };
        }
        if (raw === 'true') return { type: 'literal', value: true };
        if (raw === 'false') return { type: 'literal', value: false };

        throw new Error(`Unknown operand: ${raw}`);
    }
}
