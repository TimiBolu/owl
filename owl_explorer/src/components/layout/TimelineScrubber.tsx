
import React, { useRef, useEffect } from 'react';
import { useTimeStore } from '../../lib/time/store';
import { Play, Pause, FastForward, Rewind, SkipForward, Clock } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';

export function TimelineScrubber() {
    const history = useTimeStore(s => s.history);
    const currentTime = useTimeStore(s => s.currentTime);
    const isPaused = useTimeStore(s => s.isPaused);
    const seek = useTimeStore(s => s.seek);
    const setPaused = useTimeStore(s => s.setPaused);

    // Derived state
    const hasHistory = history.length > 1;
    const minTime = hasHistory ? history[0].timestamp : Date.now();
    const maxTime = hasHistory ? history[history.length - 1].timestamp : Date.now();

    // If live (currentTime null), we are at maxTime
    const val = currentTime === null ? maxTime : currentTime;

    // Format time for display
    const formatTime = (ts: number) => {
        return new Date(ts).toLocaleTimeString([], { hour12: false, hour: '2-digit', minute: '2-digit', second: '2-digit' });
    };

    const handleSeek = (e: React.ChangeEvent<HTMLInputElement>) => {
        const newTime = Number(e.target.value);
        setPaused(true); // Auto-pause when scrubbing
        seek(newTime);
    };

    const togglePlayPause = () => {
        if (currentTime !== null) {
            // If we are in the past and hit play, we ideally "resume" playback from there
            // For now, let's just jump to live to keep it simple, OR simple resume logic
            // "Go Live" behavior:
            seek(null);
            setPaused(false);
        } else {
            // We are live, so we pause
            setPaused(!isPaused);
        }
    };

    return (
        <div className="h-16 border-t bg-background/80 backdrop-blur-md flex items-center px-4 gap-4 z-50">
            {/* Controls */}
            <div className="flex items-center gap-1">
                <Button
                    variant="ghost"
                    size="icon"
                    className={cn("h-8 w-8", isPaused && "text-yellow-500 animate-pulse")}
                    onClick={() => setPaused(!isPaused)}
                >
                    {isPaused ? <Play className="h-4 w-4 fill-current" /> : <Pause className="h-4 w-4 fill-current" />}
                </Button>

                <Button
                    variant="ghost"
                    size="icon"
                    className="h-8 w-8 text-primary font-bold"
                    onClick={() => { seek(null); setPaused(false); }}
                    disabled={currentTime === null && !isPaused}
                >
                    <span className="text-xs">LIVE</span>
                </Button>
            </div>

            {/* Timeline Bar */}
            <div className="flex-1 flex flex-col justify-center gap-1 relative group">
                {/* Time Labels */}
                <div className="flex justify-between text-[10px] text-muted-foreground font-mono px-1">
                    <span>{formatTime(minTime)}</span>
                    <span className={cn(currentTime !== null && "text-primary font-bold")}>
                        {formatTime(val)}
                    </span>
                </div>

                {/* Slider */}
                <input
                    type="range"
                    min={minTime}
                    max={maxTime}
                    step={100} // 100ms precision
                    value={val}
                    onChange={handleSeek}
                    className="w-full h-2 bg-muted rounded-lg appearance-none cursor-pointer accent-primary hover:accent-primary/80 transition-all"
                />

                {/* Visual indicator of "buffer health" could go here (sparkline) */}
            </div>

            {/* Status */}
            <div className="flex items-center gap-2 min-w-[120px] justify-end">
                <Clock className="h-4 w-4 text-muted-foreground" />
                <div className="flex flex-col text-right">
                    <span className="text-xs font-mono font-bold">
                        {currentTime ? 'HISTORICAL' : 'REAL-TIME'}
                    </span>
                    <span className="text-[10px] text-muted-foreground">
                        {history.length} snapshots
                    </span>
                </div>
            </div>
        </div>
    );
}
