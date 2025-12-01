// Task 9.7: Memory Prefetching Hints for Navigation Operations
// Provides explicit prefetch instructions to reduce cache misses during navigation

const std = @import("std");
const builtin = @import("builtin");

/// Prefetch locality hints
pub const Locality = enum(u8) {
    /// No temporal locality (data will be used once)
    none = 0,
    /// Low temporal locality (data might be reused soon)
    low = 1,
    /// Moderate temporal locality (data likely to be reused)
    moderate = 2,
    /// High temporal locality (data will be reused multiple times)
    high = 3,
};

/// Prefetch intent hints
pub const Intent = enum {
    /// Prefetch for read access
    read,
    /// Prefetch for write access
    write,
};

/// Prefetch a cache line for reading
///
/// This function provides a hint to the CPU to prefetch the specified memory
/// address into the cache. This can significantly reduce latency for pointer
/// chasing operations like object navigation.
///
/// # Parameters
/// - `ptr`: Pointer to the memory address to prefetch
/// - `locality`: Temporal locality hint (how soon the data will be reused)
///
/// # Performance Impact
/// - Can reduce cache miss latency from ~100ns to <10ns
/// - Most effective for predictable access patterns
/// - Minimal overhead if prediction is wrong (~1-2 cycles)
pub inline fn prefetchRead(ptr: anytype, locality: Locality) void {
    const addr = @intFromPtr(ptr);

    // Only emit prefetch instructions on architectures that support them
    if (comptime builtin.cpu.arch == .x86_64) {
        // Use inline assembly for x86_64 prefetch instructions
        switch (locality) {
            .none => asm volatile ("prefetchnta (%[addr])"
                :
                : [addr] "r" (addr),
            ),
            .low => asm volatile ("prefetcht2 (%[addr])"
                :
                : [addr] "r" (addr),
            ),
            .moderate => asm volatile ("prefetcht1 (%[addr])"
                :
                : [addr] "r" (addr),
            ),
            .high => asm volatile ("prefetcht0 (%[addr])"
                :
                : [addr] "r" (addr),
            ),
        }
    } else if (comptime builtin.cpu.arch == .aarch64) {
        // ARM64 prefetch instructions
        switch (locality) {
            .none => asm volatile ("prfm pldl1strm, [%[addr]]"
                :
                : [addr] "r" (addr),
            ),
            .low => asm volatile ("prfm pldl3keep, [%[addr]]"
                :
                : [addr] "r" (addr),
            ),
            .moderate => asm volatile ("prfm pldl2keep, [%[addr]]"
                :
                : [addr] "r" (addr),
            ),
            .high => asm volatile ("prfm pldl1keep, [%[addr]]"
                :
                : [addr] "r" (addr),
            ),
        }
    }
    // For other architectures, this becomes a no-op
    // The compiler will optimize it away
}

/// Prefetch a cache line for writing
///
/// Similar to prefetchRead, but hints that the data will be written to.
/// This can trigger exclusive cache line acquisition on some architectures.
pub inline fn prefetchWrite(ptr: anytype, locality: Locality) void {
    const addr = @intFromPtr(ptr);

    if (comptime builtin.cpu.arch == .x86_64) {
        // x86_64 doesn't distinguish read/write prefetch in the instruction
        // but we can use prefetchw on AMD or prefetchwt1 on Intel
        switch (locality) {
            .none => asm volatile ("prefetchnta (%[addr])"
                :
                : [addr] "r" (addr),
            ),
            .low, .moderate, .high => asm volatile ("prefetchw (%[addr])"
                :
                : [addr] "r" (addr),
            ),
        }
    } else if (comptime builtin.cpu.arch == .aarch64) {
        // ARM64 has explicit write prefetch
        switch (locality) {
            .none => asm volatile ("prfm pstl1strm, [%[addr]]"
                :
                : [addr] "r" (addr),
            ),
            .low => asm volatile ("prfm pstl3keep, [%[addr]]"
                :
                : [addr] "r" (addr),
            ),
            .moderate => asm volatile ("prfm pstl2keep, [%[addr]]"
                :
                : [addr] "r" (addr),
            ),
            .high => asm volatile ("prfm pstl1keep, [%[addr]]"
                :
                : [addr] "r" (addr),
            ),
        }
    }
}

/// Prefetch multiple sequential cache lines
///
/// Useful for prefetching an entire object or array that spans multiple
/// cache lines. This can significantly improve throughput for sequential
/// access patterns.
///
/// # Parameters
/// - `ptr`: Starting address
/// - `size`: Number of bytes to prefetch
/// - `locality`: Temporal locality hint
pub inline fn prefetchRange(ptr: anytype, size: usize, locality: Locality) void {
    const cache_line_size = 64; // Standard cache line size
    const start_addr = @intFromPtr(ptr);
    const end_addr = start_addr + size;

    var addr = start_addr;
    while (addr < end_addr) : (addr += cache_line_size) {
        const line_ptr: *const u8 = @ptrFromInt(addr);
        prefetchRead(line_ptr, locality);
    }
}

/// Prefetch for object navigation
///
/// Specialized prefetch for object graph navigation. Prefetches the object
/// header and the beginning of the object data.
///
/// # Parameters
/// - `obj_ptr`: Pointer to the object
/// - `obj_size`: Size of the object in bytes
///
/// # Usage
/// ```zig
/// const obj_ptr = arena.basePtr() + objref.getOffset();
/// prefetchForNavigation(obj_ptr, 256); // Prefetch first 256 bytes
/// ```
pub inline fn prefetchForNavigation(obj_ptr: [*]const u8, obj_size: usize) void {
    // Prefetch object header with high locality (will be accessed immediately)
    prefetchRead(obj_ptr, .high);

    // If object is larger than one cache line, prefetch next cache line too
    if (obj_size > 64) {
        prefetchRead(obj_ptr + 64, .moderate);
    }

    // If object is very large, prefetch third cache line
    if (obj_size > 128) {
        prefetchRead(obj_ptr + 128, .low);
    }
}

/// Prefetch for reference chasing
///
/// When navigating through object references, prefetch the target object
/// before actually dereferencing. This hides memory latency.
///
/// # Parameters
/// - `base_ptr`: Base pointer of the arena
/// - `offset`: Offset of the target object
///
/// # Usage
/// ```zig
/// // Before dereferencing a reference
/// const target_offset = objref.getOffset();
/// prefetchForReferenceChase(arena.basePtr(), target_offset);
/// // ... do some other work ...
/// const target_obj = arena.read(objref); // Cache miss avoided!
/// ```
pub inline fn prefetchForReferenceChase(base_ptr: [*]const u8, offset: u64) void {
    const target_ptr = base_ptr + offset;
    // High locality because we're about to access it
    prefetchRead(target_ptr, .high);
}

/// Prefetch multiple references in parallel
///
/// When you know you'll be accessing multiple references, prefetch them
/// all at once to maximize memory-level parallelism.
///
/// # Parameters
/// - `base_ptr`: Base pointer of the arena
/// - `offsets`: Array of offsets to prefetch
pub inline fn prefetchMultipleReferences(base_ptr: [*]const u8, offsets: []const u64) void {
    for (offsets) |offset| {
        const target_ptr = base_ptr + offset;
        prefetchRead(target_ptr, .moderate);
    }
}

/// Software prefetch distance calculator
///
/// Calculates the optimal prefetch distance based on memory latency and
/// computation time. This helps determine how far ahead to prefetch.
///
/// # Parameters
/// - `memory_latency_ns`: Expected memory latency in nanoseconds (~100ns for DRAM)
/// - `computation_time_ns`: Time spent computing between accesses
///
/// # Returns
/// Number of iterations to prefetch ahead
pub fn calculatePrefetchDistance(memory_latency_ns: u64, computation_time_ns: u64) usize {
    if (computation_time_ns == 0) return 1;
    const distance = (memory_latency_ns + computation_time_ns - 1) / computation_time_ns;
    return @intCast(@min(distance, 16)); // Cap at 16 to avoid over-prefetching
}

/// Prefetch with stride pattern
///
/// For strided access patterns (e.g., accessing every Nth element),
/// prefetch ahead with the same stride.
///
/// # Parameters
/// - `base_ptr`: Starting address
/// - `stride`: Stride in bytes
/// - `count`: Number of elements to prefetch
/// - `locality`: Temporal locality hint
pub inline fn prefetchStrided(base_ptr: [*]const u8, stride: usize, count: usize, locality: Locality) void {
    var i: usize = 0;
    while (i < count) : (i += 1) {
        const ptr = base_ptr + (i * stride);
        prefetchRead(ptr, locality);
    }
}

// ============================================================================
// Tests
// ============================================================================

test "prefetch: basic read prefetch" {
    var data: [1024]u8 = undefined;

    // These should compile and not crash
    prefetchRead(&data[0], .high);
    prefetchRead(&data[64], .moderate);
    prefetchRead(&data[128], .low);
    prefetchRead(&data[192], .none);
}

test "prefetch: basic write prefetch" {
    var data: [1024]u8 = undefined;

    // These should compile and not crash
    prefetchWrite(&data[0], .high);
    prefetchWrite(&data[64], .moderate);
    prefetchWrite(&data[128], .low);
    prefetchWrite(&data[192], .none);
}

test "prefetch: range prefetch" {
    var data: [1024]u8 = undefined;

    // Prefetch entire array
    prefetchRange(&data[0], data.len, .moderate);

    // Prefetch partial range
    prefetchRange(&data[256], 256, .high);
}

test "prefetch: navigation prefetch" {
    var data: [1024]u8 = undefined;

    // Small object (< 64 bytes)
    prefetchForNavigation(@ptrCast(&data), 32);

    // Medium object (64-128 bytes)
    prefetchForNavigation(@ptrCast(&data), 96);

    // Large object (> 128 bytes)
    prefetchForNavigation(@ptrCast(&data), 256);
}

test "prefetch: reference chase" {
    var arena: [4096]u8 = undefined;
    const base_ptr: [*]u8 = @ptrCast(&arena);

    // Prefetch object at offset 1000
    prefetchForReferenceChase(base_ptr, 1000);

    // Prefetch object at offset 2000
    prefetchForReferenceChase(base_ptr, 2000);
}

test "prefetch: multiple references" {
    var arena: [4096]u8 = undefined;
    const base_ptr: [*]u8 = @ptrCast(&arena);

    const offsets = [_]u64{ 100, 200, 300, 400, 500 };
    prefetchMultipleReferences(base_ptr, &offsets);
}

test "prefetch: distance calculation" {
    // Memory latency 100ns, computation 10ns -> prefetch 10 ahead
    const dist1 = calculatePrefetchDistance(100, 10);
    try std.testing.expectEqual(@as(usize, 10), dist1);

    // Memory latency 100ns, computation 50ns -> prefetch 2 ahead
    const dist2 = calculatePrefetchDistance(100, 50);
    try std.testing.expectEqual(@as(usize, 2), dist2);

    // Memory latency 100ns, computation 100ns -> prefetch 1 ahead
    const dist3 = calculatePrefetchDistance(100, 100);
    try std.testing.expectEqual(@as(usize, 1), dist3);

    // Very fast computation -> cap at 16
    const dist4 = calculatePrefetchDistance(1000, 1);
    try std.testing.expectEqual(@as(usize, 16), dist4);
}

test "prefetch: strided access" {
    var data: [1024]u8 = undefined;
    const base_ptr: [*]u8 = @ptrCast(&data);

    // Prefetch every 64 bytes (cache line aligned)
    prefetchStrided(base_ptr, 64, 8, .moderate);

    // Prefetch every 128 bytes
    prefetchStrided(base_ptr, 128, 4, .low);
}

test "prefetch: pointer types" {
    var data: [1024]u8 = undefined;

    // Test with different pointer types
    const ptr1: *u8 = &data[0];
    prefetchRead(ptr1, .high);

    const ptr2: [*]u8 = @ptrCast(&data);
    prefetchRead(ptr2, .high);

    const ptr3: *const u8 = &data[0];
    prefetchRead(ptr3, .high);
}
