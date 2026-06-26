
import { useEffect, useState, useCallback } from "react";
import { audio } from "@/lib/audio/engine";
import { useMetrics } from "@/hooks/useMetrics";
import { useTimeStore } from "@/lib/time/store";

export function useJarvis() {
    const { data } = useMetrics();
    const [isMuted, setIsMuted] = useState(false);
    const [isListening, setIsListening] = useState(false);

    const setPaused = useTimeStore(s => s.setPaused);
    const seek = useTimeStore(s => s.seek);



    // Sonification Loop - DISABLED per user request
    /*
    useEffect(() => {
        if (isMuted) return;
        // ... code removed ...
    }, [isMuted, data.active_alerts.length]);
    */




    // Initialize Audio on First Interaction
    const initAudio = useCallback(() => {
        audio.init();
    }, []);

    const toggleMute = useCallback(() => {
        const newState = !isMuted;
        setIsMuted(newState);
        audio.toggleMute(newState);
    }, [isMuted]);

    // Voice Command Setup
    useEffect(() => {
        if (!('webkitSpeechRecognition' in window)) return;

        // Typings for Web Speech API are often missing in standard lib
        const SpeechRecognition = (window as any).webkitSpeechRecognition;
        const recognition = new SpeechRecognition();
        recognition.continuous = true;
        recognition.interimResults = false;
        recognition.lang = 'en-US';

        recognition.onstart = () => setIsListening(true);
        recognition.onend = () => setIsListening(false);

        recognition.onresult = (event: any) => {
            const last = event.results.length - 1;
            const command = event.results[last][0].transcript.trim().toLowerCase();

            console.log("Jarvis heard:", command);

            if (command.includes("owl pause") || command.includes("stop")) {
                setPaused(true);
                audio.playClick();
            } else if (command.includes("owl play") || command.includes("resume")) {
                setPaused(false);
                audio.playClick();
            } else if (command.includes("go live")) {
                seek(null);
                setPaused(false);
                audio.playGlitch(); // Specific SFX for time jump
            } else if (command.includes("silence") || command.includes("mute")) {
                toggleMute();
            }
        };

        // Auto-start listening (in a real app, this should be toggled by user)
        // recognition.start(); 

        return () => {
            // recognition.stop();
        };
    }, [setPaused, seek, toggleMute]);

    return {
        isMuted,
        toggleMute,
        initAudio,
        isListening
    };
}
