
import { create } from 'zustand';

// Re-export metrics type if needed, or define a compatible shape
export interface TimeSeriesPoint {
    timestamp: number;
    metrics: any; // Using any to avoid circular dependency with useMetrics for now
}

interface TimeState {
    history: TimeSeriesPoint[];
    currentTime: number | null; // null means "Live"
    isPaused: boolean;
    playbackSpeed: number; // 1 = 1x real-time

    // Actions
    addSnapshot: (metrics: any) => void;
    seek: (timestamp: number | null) => void;
    setPaused: (paused: boolean) => void;
    setPlaybackSpeed: (speed: number) => void;
    clearHistory: () => void;
}

const MAX_HISTORY_LENGTH = 600; // Store last ~10 mins (assuming 1s interval)

export const useTimeStore = create<TimeState>((set) => ({
    history: [],
    currentTime: null,
    isPaused: false,
    playbackSpeed: 1,

    addSnapshot: (metrics) => set((state) => {
        // If we are paused or scrubbing, we ideally shouldn't be adding new live data 
        // OR we add it but don't move the "cursor" if it's locked. 
        // For simplicity: always record in background.

        const point: TimeSeriesPoint = {
            timestamp: Date.now(),
            metrics
        };

        const newHistory = [...state.history, point];
        if (newHistory.length > MAX_HISTORY_LENGTH) {
            newHistory.shift();
        }

        return { history: newHistory };
    }),

    seek: (timestamp) => set({ currentTime: timestamp }),

    setPaused: (paused) => set({ isPaused: paused }),

    setPlaybackSpeed: (speed) => set({ playbackSpeed: speed }),

    clearHistory: () => set({ history: [], currentTime: null })
}));
