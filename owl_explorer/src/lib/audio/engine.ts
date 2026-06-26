
// Simple Web Audio API wrapper for Sci-Fi UI sounds

export class AudioEngine {
    private ctx: AudioContext | null = null;
    private droneOsc: OscillatorNode | null = null;
    private droneGain: GainNode | null = null;
    private isMuted: boolean = false;
    private isInitialized: boolean = false;

    constructor() {
        // We init lazily on first interaction
    }

    public init() {
        if (this.isInitialized) return;

        try {
            const AudioContextClass = window.AudioContext || (window as any).webkitAudioContext;
            this.ctx = new AudioContextClass();
            this.isInitialized = true;
            this.startDrone();
        } catch (e) {
            console.error("Web Audio API not supported", e);
        }
    }

    private startDrone() {
        if (!this.ctx) return;

        // Create a low frequency "hum"
        this.droneOsc = this.ctx.createOscillator();
        this.droneGain = this.ctx.createGain();

        this.droneOsc.type = 'sawtooth';
        this.droneOsc.frequency.setValueAtTime(50, this.ctx.currentTime); // 50Hz Low Hum

        // Bandpass filter to make it sound "distant"
        const filter = this.ctx.createBiquadFilter();
        filter.type = 'lowpass';
        filter.frequency.value = 200;

        this.droneOsc.connect(filter);
        filter.connect(this.droneGain);
        this.droneGain.connect(this.ctx.destination);

        this.droneGain.gain.setValueAtTime(0.02, this.ctx.currentTime); // Very quiet
        this.droneOsc.start();
    }


    public updateDrone(load: number) {
        // User requested silence. Drone is disabled.
        if (!this.ctx || this.isMuted) return;
    }

    public playHoot() {
        if (!this.ctx || this.isMuted) return;

        const t = this.ctx.currentTime;
        const osc = this.ctx.createOscillator();
        const gain = this.ctx.createGain();

        osc.connect(gain);
        gain.connect(this.ctx.destination);

        // Owl Hoot Synthesis: Sine wave with pitch drop
        osc.type = 'sine';
        osc.frequency.setValueAtTime(600, t);
        osc.frequency.exponentialRampToValueAtTime(400, t + 0.2);

        // Envelope: "Hoo"
        gain.gain.setValueAtTime(0, t);
        gain.gain.linearRampToValueAtTime(0.5, t + 0.05);
        gain.gain.exponentialRampToValueAtTime(0.01, t + 0.4);

        osc.start(t);
        osc.stop(t + 0.5);

        // Second Hoot (lower, longer)
        const osc2 = this.ctx.createOscillator();
        const gain2 = this.ctx.createGain();
        osc2.connect(gain2);
        gain2.connect(this.ctx.destination);

        osc2.type = 'sine';
        osc2.frequency.setValueAtTime(400, t + 0.6);
        osc2.frequency.exponentialRampToValueAtTime(300, t + 1.2);

        gain2.gain.setValueAtTime(0, t + 0.6);
        gain2.gain.linearRampToValueAtTime(0.4, t + 0.7);
        gain2.gain.exponentialRampToValueAtTime(0.01, t + 1.5);

        osc2.start(t + 0.6);
        osc2.stop(t + 1.5);
    }


    public playGlitch() {
        if (!this.ctx || this.isMuted) return;
        const osc = this.ctx.createOscillator();
        const gain = this.ctx.createGain();

        osc.frequency.setValueAtTime(100 + Math.random() * 500, this.ctx.currentTime);
        osc.type = 'square';

        gain.gain.setValueAtTime(0.05, this.ctx.currentTime);
        gain.gain.exponentialRampToValueAtTime(0.001, this.ctx.currentTime + 0.1);

        osc.connect(gain);
        gain.connect(this.ctx.destination);
        osc.start();
        osc.stop(this.ctx.currentTime + 0.1);
    }

    public playClick() {
        if (!this.ctx || this.isMuted) return;
        const osc = this.ctx.createOscillator();
        const gain = this.ctx.createGain();

        osc.frequency.setValueAtTime(800, this.ctx.currentTime);
        osc.type = 'sine';

        gain.gain.setValueAtTime(0.03, this.ctx.currentTime);
        gain.gain.exponentialRampToValueAtTime(0.001, this.ctx.currentTime + 0.05);

        osc.connect(gain);
        gain.connect(this.ctx.destination);
        osc.start();
        osc.stop(this.ctx.currentTime + 0.05);
    }

    public toggleMute(muted: boolean) {
        this.isMuted = muted;
        if (this.ctx && this.droneGain) {
            this.droneGain.gain.setTargetAtTime(muted ? 0 : 0.02, this.ctx.currentTime, 0.1);
        }
        if (this.ctx?.state === 'suspended' && !muted) {
            this.ctx.resume();
        }
    }
}

export const audio = new AudioEngine();
