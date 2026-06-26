const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    // Create storage module
    const storage_mod = b.addModule("storage", .{
        .root_source_file = b.path("src/root.zig"),
        .target = target,
    });

    // Unit tests
    const tests = b.addTest(.{
        .root_module = b.createModule(.{
            .root_source_file = b.path("src/test_all.zig"),
            .target = target,
            .optimize = optimize,
        }),
    });
    tests.linkLibC();
    tests.linkSystemLibrary("lz4");
    tests.linkSystemLibrary("zstd");
    const run_tests = b.addRunArtifact(tests);
    const test_step = b.step("test", "Run unit tests");
    test_step.dependOn(&run_tests.step);

    // Benchmark executable
    const bench = b.addExecutable(.{
        .name = "bench",
        .root_module = b.createModule(.{
            .root_source_file = b.path("benches/bench_all.zig"),
            .target = target,
            .optimize = .ReleaseFast,
            .imports = &.{
                .{ .name = "storage", .module = storage_mod },
            },
        }),
    });
    b.installArtifact(bench);

    const run_bench = b.addRunArtifact(bench);
    const bench_step = b.step("bench", "Run benchmarks");
    bench_step.dependOn(&run_bench.step);

    // B-tree benchmark executable
    const btree_bench = b.addExecutable(.{
        .name = "btree_bench",
        .root_module = b.createModule(.{
            .root_source_file = b.path("benches/btree_bench.zig"),
            .target = target,
            .optimize = .ReleaseFast,
            .imports = &.{
                .{ .name = "storage", .module = storage_mod },
            },
        }),
    });
    b.installArtifact(btree_bench);

    const run_btree_bench = b.addRunArtifact(btree_bench);
    const btree_bench_step = b.step("bench-btree", "Run B-tree benchmarks");
    btree_bench_step.dependOn(&run_btree_bench.step);

    // Lock-free WAL benchmark executable (Task 9.4)
    const lockfree_wal_bench = b.addExecutable(.{
        .name = "lockfree_wal_bench",
        .root_module = b.createModule(.{
            .root_source_file = b.path("benches/lockfree_wal_bench.zig"),
            .target = target,
            .optimize = .ReleaseFast,
            .imports = &.{
                .{ .name = "storage", .module = storage_mod },
            },
        }),
    });
    b.installArtifact(lockfree_wal_bench);

    const run_lockfree_wal_bench = b.addRunArtifact(lockfree_wal_bench);
    const lockfree_wal_bench_step = b.step("bench-lockfree-wal", "Run lock-free WAL benchmarks");
    lockfree_wal_bench_step.dependOn(&run_lockfree_wal_bench.step);

    // WalIntentRing WAL benchmark
    const wal_intent_ring_bench = b.addExecutable(.{
        .name = "wal_intent_ring_bench",
        .root_module = b.createModule(.{
            .root_source_file = b.path("benches/wal_intent_ring_bench.zig"),
            .target = target,
            .optimize = .ReleaseFast,
            .imports = &.{
                .{ .name = "storage", .module = storage_mod },
            },
        }),
    });
    b.installArtifact(wal_intent_ring_bench);

    const run_wal_intent_ring_bench = b.addRunArtifact(wal_intent_ring_bench);
    const wal_intent_ring_bench_step = b.step("bench-wal-intent-ring", "Run WalIntentRing WAL benchmarks");
    wal_intent_ring_bench_step.dependOn(&run_wal_intent_ring_bench.step);

    // SIMD operations benchmark executable (Task 9.8)
    const simd_bench = b.addExecutable(.{
        .name = "simd_bench",
        .root_module = b.createModule(.{
            .root_source_file = b.path("benches/simd_bench.zig"),
            .target = target,
            .optimize = .ReleaseFast,
            .imports = &.{
                .{ .name = "storage", .module = storage_mod },
                .{ .name = "simd_ops", .module = b.addModule("simd_ops", .{
                    .root_source_file = b.path("src/simd_ops.zig"),
                    .target = target,
                }) },
            },
        }),
    });
    b.installArtifact(simd_bench);

    const run_simd_bench = b.addRunArtifact(simd_bench);
    const simd_bench_step = b.step("bench-simd", "Run SIMD operations benchmarks");
    simd_bench_step.dependOn(&run_simd_bench.step);

    // Static library for FFI
    const lib = b.addLibrary(.{
        .name = "owl_storage",
        .linkage = .static,
        .root_module = b.createModule(.{
            .root_source_file = b.path("src/c_api.zig"),
            .target = target,
            .optimize = optimize,
        }),
    });
    lib.linkLibC();
    lib.linkSystemLibrary("lz4");
    lib.linkSystemLibrary("zstd");
    b.installArtifact(lib);

    // Install header file
    const install_header = b.addInstallFile(
        b.path("include/owl_storage.h"),
        "include/owl_storage.h",
    );
    b.getInstallStep().dependOn(&install_header.step);

    // Crash simulation demo
    const crash_demo = b.addExecutable(.{
        .name = "crash_demo",
        .root_module = b.createModule(.{
            .root_source_file = b.path("examples/crash_simulation_demo.zig"),
            .target = target,
            .optimize = optimize,
            .imports = &.{
                .{ .name = "storage", .module = storage_mod },
            },
        }),
    });
    b.installArtifact(crash_demo);

    const run_crash_demo = b.addRunArtifact(crash_demo);
    const crash_demo_step = b.step("demo", "Run crash simulation demo");
    crash_demo_step.dependOn(&run_crash_demo.step);

    // Distributed arena demo
    const distributed_demo = b.addExecutable(.{
        .name = "distributed_demo",
        .root_module = b.createModule(.{
            .root_source_file = b.path("examples/distributed_arena_demo.zig"),
            .target = target,
            .optimize = optimize,
            .imports = &.{
                .{ .name = "storage", .module = storage_mod },
            },
        }),
    });
    b.installArtifact(distributed_demo);

    const run_distributed_demo = b.addRunArtifact(distributed_demo);
    const distributed_demo_step = b.step("demo-distributed", "Run distributed arena demo");
    distributed_demo_step.dependOn(&run_distributed_demo.step);

    // Distributed arena management demo
    const distributed_mgmt_demo = b.addExecutable(.{
        .name = "distributed_mgmt_demo",
        .root_module = b.createModule(.{
            .root_source_file = b.path("examples/distributed_arena_management_demo.zig"),
            .target = target,
            .optimize = optimize,
            .imports = &.{
                .{ .name = "storage", .module = storage_mod },
            },
        }),
    });
    b.installArtifact(distributed_mgmt_demo);

    const run_distributed_mgmt_demo = b.addRunArtifact(distributed_mgmt_demo);
    const distributed_mgmt_demo_step = b.step("demo-distributed-mgmt", "Run distributed arena management demo");
    distributed_mgmt_demo_step.dependOn(&run_distributed_mgmt_demo.step);

    // SIMD operations demo (Task 9.8)
    const simd_demo = b.addExecutable(.{
        .name = "simd_demo",
        .root_module = b.createModule(.{
            .root_source_file = b.path("examples/simd_demo.zig"),
            .target = target,
            .optimize = optimize,
            .imports = &.{
                .{ .name = "storage", .module = storage_mod },
            },
        }),
    });
    b.installArtifact(simd_demo);

    const run_simd_demo = b.addRunArtifact(simd_demo);
    const simd_demo_step = b.step("demo-simd", "Run SIMD operations demo");
    simd_demo_step.dependOn(&run_simd_demo.step);

    // Memory manager demo (Task 9.10)
    const memory_manager_demo = b.addExecutable(.{
        .name = "memory_manager_demo",
        .root_module = b.createModule(.{
            .root_source_file = b.path("examples/memory_manager_demo.zig"),
            .target = target,
            .optimize = optimize,
            .imports = &.{
                .{ .name = "storage", .module = storage_mod },
            },
        }),
    });
    b.installArtifact(memory_manager_demo);

    const run_memory_manager_demo = b.addRunArtifact(memory_manager_demo);
    const memory_manager_demo_step = b.step("demo-memory-manager", "Run memory manager demo");
    memory_manager_demo_step.dependOn(&run_memory_manager_demo.step);

    // Compression benchmark (Task 10.4)
    const compression_bench = b.addExecutable(.{
        .name = "compression_bench",
        .root_module = b.createModule(.{
            .root_source_file = b.path("benches/compression_bench.zig"),
            .target = target,
            .optimize = .ReleaseFast,
            .imports = &.{
                .{ .name = "storage", .module = storage_mod },
            },
        }),
    });
    compression_bench.linkLibC();
    compression_bench.linkSystemLibrary("lz4");
    compression_bench.linkSystemLibrary("zstd");
    b.installArtifact(compression_bench);

    const run_compression_bench = b.addRunArtifact(compression_bench);
    const compression_bench_step = b.step("bench-compression", "Run compression benchmarks");
    compression_bench_step.dependOn(&run_compression_bench.step);
}
