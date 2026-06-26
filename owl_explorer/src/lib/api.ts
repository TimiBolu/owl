
export async function api<T>(path: string, body?: unknown): Promise<T> {
    const res = await fetch(path, {
        method: body ? "POST" : "GET",
        headers: body ? { "Content-Type": "application/json" } : undefined,
        body: body ? JSON.stringify(body) : undefined,
    });
    const data = await res.json();
    if (!res.ok || data.ok === false) {
        throw new Error(data.error ?? `HTTP ${res.status}`);
    }
    return data as T;
}

export function presentErrorMessage(message: string): string {
    if (message.includes("InvalidArena")) {
        return `Storage arena could not be opened (InvalidArena). The DB file may be invalid/corrupt or sized incorrectly. ${message}`;
    }
    return message;
}
