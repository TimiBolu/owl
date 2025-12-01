const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const exe = b.addExecutable(.{
        .name = "deterministic_allocation_demo",
        .target = target,
        .optimize = optimize,
    });
    exe.addSourceFile(.{ .file = b.path("deterministic_allocation_demo.zig") });

    // Add the src directory as a module
    const owl_storage = b.addModule("owl_storage", .{
        .root_source_file = b.path("../src/root.zig"),
    });
    exe.root_module.addImport("owl_storage", owl_storage);

    b.installArtifact(exe);

    const run_cmd = b.addRunArtifact(exe);
    run_cmd.step.dependOn(b.getInstallStep());

    const run_step = b.step("run", "Run the demo");
    run_step.dependOn(&run_cmd.step);
}
