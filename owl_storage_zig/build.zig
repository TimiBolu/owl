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
    b.installArtifact(lib);

    // Install header file
    const install_header = b.addInstallFile(
        b.path("include/owl_storage.h"),
        "include/owl_storage.h",
    );
    b.getInstallStep().dependOn(&install_header.step);
}
