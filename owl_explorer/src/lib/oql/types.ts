
export type OQLExpression =
    | { type: 'binary', operator: string, left: OQLExpression, right: OQLExpression }
    | { type: 'property', name: string }
    | { type: 'literal', value: string | number | boolean }
    | { type: 'function_call', name: string, args: OQLExpression[] };

export interface OQLQuery {
    collection: string;
    where?: OQLExpression;
    select?: string[];
    limit?: number;
}
