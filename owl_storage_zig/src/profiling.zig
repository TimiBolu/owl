// Performance Profiling for Zig Storage Layer

const std = @import("std");

/// Profile sample for FFI
pub const ProfileSample = extern struct {
    function_name: [*]const u8,
    function_name_len: usize,
    file: [*]const u8,
    file_len: usize,
    line: u32,
    duration_ns: u64,
};

/// Profiled function execution
pub const ProfiledFunction = struct {
    function_name: []const u8,
    file: []const u8,
    line: u32,
    start_ns: u64,

    const Self = @This();

    /// Start profiling a function
    pub fn start(function_name: []const u8, file: []const u8, line: u32) Self {
        return Self{
            .function_name = function_name,
            .file = file,
            .line = line,
            .start_ns = @intCast(std.time.nanoTimestamp()),
        };
    }

    /// Finish profiling and record sample
    pub fn finish(self: *const Self, profiler: *anyopaque) void {
        const end_ns: u64 = @intCast(std.time.nanoTimestamp());
        const duration_ns = end_ns - self.start_ns;

        const sample = ProfileSample{
            .function_name = self.function_name.ptr,
            .function_name_len = self.function_name.len,
            .file = self.file.ptr,
            .file_len = self.file.len,
            .line = self.line,
            .duration_ns = duration_ns,
        };

        owl_profile_record_sample(profiler, &sample);
    }
};

/// Record an allocation for profiling
pub fn recordAllocation(profiler: *anyopaque, location: []const u8, size: u64) void {
    owl_profile_record_allocation(profiler, location.ptr, location.len, size);
}

// FFI declarations
extern "C" fn owl_profile_record_sample(profiler: *anyopaque, sample: *const ProfileSample) void;
extern "C" fn owl_profile_record_allocation(profiler: *anyopaque, location: [*]const u8, location_len: usize, size: u64) void;

test "profiled function" {
    // Note: This test doesn't actually call FFI since we don't have a profiler
    const func = ProfiledFunction.start("test_function", "test.zig", 42);

    // Simulate some work
    var sum: u64 = 0;
    var i: u64 = 0;
    while (i < 1000) : (i += 1) {
        sum +%= i;
    }

    // In production, this would call FFI
    // func.finish(profiler);
    _ = func;
}
