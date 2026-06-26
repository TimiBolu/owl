
import { Canvas, useFrame } from '@react-three/fiber';
import { OrbitControls, Stars, Text } from '@react-three/drei';
import { useRef, useState, useMemo } from 'react';
import * as THREE from 'three';
import { useChaosStore } from '@/lib/chaos/store';
import { useClusterTopology } from '@/hooks/useClusterTopology';

// A single node in the cluster
function NodeMesh({ position, label, isTarget, isDead, load }: { position: [number, number, number], label: string, isTarget: boolean, isDead: boolean, load: number }) {
    const meshRef = useRef<THREE.Mesh>(null);
    const [hovered, setHover] = useState(false);

    useFrame((state, delta) => {
        if (meshRef.current) {
            meshRef.current.rotation.x += delta * 0.2;
            meshRef.current.rotation.y += delta * 0.1;

            // Pulse if under load
            if (load > 0.5) {
                const s = 1 + Math.sin(state.clock.elapsedTime * 10) * 0.1;
                meshRef.current.scale.set(s, s, s);
            }
        }
    });

    const color = isDead ? '#ef4444' : (isTarget ? '#eab308' : (hovered ? '#3b82f6' : '#22c55e'));

    // Dead nodes shatter (or disappear for now)
    if (isDead) return null;

    return (
        <group position={position}>
            <mesh
                ref={meshRef}
                onPointerOver={() => setHover(true)}
                onPointerOut={() => setHover(false)}
            >
                <icosahedronGeometry args={[1, 0]} />
                <meshStandardMaterial color={color} wireframe={hovered} />
            </mesh>
            <Text
                position={[0, 1.5, 0]}
                fontSize={0.3}
                color="white"
                anchorX="center"
                anchorY="middle"
            >
                {label}
            </Text>
            {load > 0.1 && (
                <Text position={[0, -1.5, 0]} fontSize={0.2} color="#fbbf24">
                    {(load * 100).toFixed(0)}% CPU
                </Text>
            )}
        </group>
    );
}

// Particle system for traffic
function TrafficParticles({ count = 100 }) {
    const points = useRef<THREE.Points>(null);

    const particles = useMemo(() => {
        const temp = new Float32Array(count * 3);
        for (let i = 0; i < count; i++) {
            const x = (Math.random() - 0.5) * 10;
            const y = (Math.random() - 0.5) * 10;
            const z = (Math.random() - 0.5) * 10;
            temp[i * 3] = x;
            temp[i * 3 + 1] = y;
            temp[i * 3 + 2] = z;
        }
        return temp;
    }, [count]);

    useFrame((state, delta) => {
        if (points.current) {
            points.current.rotation.y += delta * 0.05;
        }
    });

    return (
        <points ref={points}>


            <bufferGeometry>
                <bufferAttribute
                    attach="attributes-position"
                    args={[particles, 3]}
                />
            </bufferGeometry>


            <pointsMaterial size={0.05} color="#60a5fa" transparent opacity={0.6} />
        </points>
    );
}

export default function ClusterHolodeck() {
    const activeEvents = useChaosStore(s => s.activeEvents);
    const { nodes: topologyNodes } = useClusterTopology();

    const nodes = topologyNodes.map((node, idx) => {
        const angle = (Math.PI * 2 * idx) / Math.max(topologyNodes.length, 1);
        const radius = 4;
        return {
            id: node.id,
            load: Math.max(0, Math.min(1, (node.metrics?.cpu ?? 0) / 100)),
            pos: [Math.cos(angle) * radius, 0, Math.sin(angle) * radius] as [number, number, number],
        };
    });

    return (
        <div className="h-full w-full bg-black">
            <Canvas camera={{ position: [0, 5, 10], fov: 60 }}>
                <color attach="background" args={['#000']} />

                <Stars radius={100} depth={50} count={5000} factor={4} saturation={0} fade speed={1} />
                <ambientLight intensity={0.5} />
                <pointLight position={[10, 10, 10]} />

                <group>
                    {nodes.map((n, i) => {
                        const isDead = activeEvents.some(e => e.targetNodeId === n.id && e.type === 'meteor');
                        const isTarget = activeEvents.some(e => e.targetNodeId === n.id && e.type !== 'meteor');
                        return (
                            <NodeMesh
                                key={n.id}
                                position={n.pos}
                                label={n.id}
                                isTarget={isTarget}
                                isDead={isDead}
                                load={n.load}
                            />
                        );
                    })}
                </group>

                <TrafficParticles />

                <OrbitControls enablePan={true} enableZoom={true} enableRotate={true} />

                <gridHelper args={[20, 20, 0x222222, 0x111111]} position={[0, -2, 0]} />
            </Canvas>
        </div>
    );
}
