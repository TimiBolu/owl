
import { create } from 'zustand';

export type ChaosType = 'meteor' | 'latency' | 'fire' | 'partition';

export interface ChaosEvent {
    id: string;
    type: ChaosType;
    targetNodeId: string;
    timestamp: number;
    active: boolean;
}

interface ChaosState {
    isChaosMode: boolean;
    activeEvents: ChaosEvent[];
    selectedWeapon: ChaosType | null;

    // Actions
    toggleChaosMode: () => void;
    selectWeapon: (weapon: ChaosType | null) => void;
    triggerChaos: (nodeId: string, type: ChaosType) => void;
    resolveChaos: (eventId: string) => void;
    clearAll: () => void;
}

export const useChaosStore = create<ChaosState>((set) => ({
    isChaosMode: false,
    activeEvents: [],
    selectedWeapon: null,

    toggleChaosMode: () => set((state) => ({
        isChaosMode: !state.isChaosMode,
        selectedWeapon: null // Reset weapon on toggle
    })),

    selectWeapon: (weapon) => set({ selectedWeapon: weapon }),

    triggerChaos: (nodeId, type) => set((state) => {
        const newEvent: ChaosEvent = {
            id: crypto.randomUUID(),
            type,
            targetNodeId: nodeId,
            timestamp: Date.now(),
            active: true
        };
        return { activeEvents: [...state.activeEvents, newEvent] };
    }),

    resolveChaos: (eventId) => set((state) => ({
        activeEvents: state.activeEvents.filter(e => e.id !== eventId)
    })),

    clearAll: () => set({ activeEvents: [], selectedWeapon: null })
}));
