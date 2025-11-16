# Design Document: Zig Storage Engine for OQL

## Overview

This document specifies the Zig implementation of the OQL storage engine **core primitives**, focusing on **O(1) object graph navigation** through physical pointers. The design prioritizes performance, memory efficiency, and clean integration with the Rust query layer.

**Core Innovation**: `order.customer.profile.name` executes as 2-3 pointer dereferences (~50ns), not a database query.

**Scope**: This storage engine provides **low-level primitives** only:
- Physical object storage (ObjRef, ObjectHeader, Object)
- Memory-mapped arena allocation
- O(1) pointer swizzling and navigation
- Write-ahead logging for durability
- MVCC transactions

**Out of Scope** (handled by Rust query layer):
- Collections and document abstractions
- Schema management and validation
- Query planning and optimization
- Index management
- High-level API (OQL syntax)

The Rust layer builds collections, documents, and schemas **on top** of these storage primitives.

## Architecture

### System Layers

```
┌─────────────────────────────────────────────────────────┐
│   Rust: OQL Query Layer                                 │
│   - OQL parser (order.customer.name → path)            │
│   - Query planner & optimizer                           │
│   - Index management                                    │
│   - API layer                                           │
└────────────────────┬────────────────────────────────────┘
                     │ C ABI (function calls)
┌────────────────────▼────────────────────────────────────┐
│   Zig: Storage Engine Core                              │
│   - ObjRef swizzling (offset → pointer)                │
│   - Arena allocation with co-location                   │
│   - Object serialization                                │
│   - WAL for durability                                  │
│   - Cache management                                    │
└────────────────────┬────────────────────────────────────┘
                     │ mmap / file I/O
┌────────────────────▼────────────────────────────────────┐
│   Operating System                                       │
│   - Page cache                                          │
│   - File system                                         │
└─────────────────────────────────────────────────────────┘
```

### Component Interaction

**Navigation Path** (Hot Path - Must be Fast):
```
1. Rust: order.customer (path navigation)
2. C ABI: owl_get_reference(order, "customer")
3. Zig: Load ObjRef from object field
4. Zig: Swizzle ObjRef → memory pointer
5. Return: Pointer to customer object
Time: ~50ns (2-3 cache line loads)
```

**Write Path**:
```
1. Rust: Create/modify object
2. C ABI: owl_arena_alloc(size, near)
3. Zig: Allocate space (co-located)
4. C ABI: owl_arena_write(ref, data)
5. Zig: Write to mmap
6. Zig: Append to WAL
7. Return: ObjRef
```

## Core Data Structures

### 1. ObjRef - Physical Pointer

**Purpose**: 64-bit file offset that can be swizzled to memory pointer.

**Zig Implementation**:


```zig
pub const ObjRef = extern struct {
    offset: u64,

    pub inline fn init(offset: u64) ObjRef {
        return .{ .offset = offset };
    }

    pub inline fn null() ObjRef {
        return .{ .offset = 0 };
    }

    pub inline fn isNull(self: ObjRef) bool {
        return self.offset == 0;
    }

    pub inline fn pageId(self: ObjRef) u32 {
        return @intCast(self.offset / 4096);
    }

    pub inline fn slotId(self: ObjRef) u16 {
        return @intCast((self.offset % 4096) / 64);
    }

    // Swizzle: Convert offset to pointer (O(1))
    pub inline fn swizzle(self: ObjRef, base: [*]u8) [*]u8 {
        return base + self.offset;
    }
};
```

**Key Design Decisions**:
- `extern struct` for C ABI compatibility
- `inline` functions for zero-cost abstractions
- Simple arithmetic for page/slot calculations
- No allocations, no error handling (pure computation)

### 2. ObjectHeader - Fixed Layout

**Purpose**: Cache-efficient header with metadata.

**Zig Implementation**:
```zig
pub const ObjectHeader = extern struct {
    type_tag: u16,      // Schema type (0=User, 1=Order, etc.)
    id: [16]u8,         // UUID as bytes
    version: u32,       // MVCC version
    size: u32,          // Total object size
    flags: u8,          // Deleted, Pinned, Overflow
    checksum: u32,      // CRC32
    lsn: u64,           // Log sequence number
    
    // Total: 41 bytes (fits in single cache line with padding)
    
    pub fn init(type_tag: u16, id: [16]u8, size: u32) ObjectHeader {
        return .{
            .type_tag = type_tag,
            .id = id,
            .version = 0,
            .size = size,
            .flags = 0,
            .checksum = 0,
            .lsn = 0,
        };
    }
    
    pub inline fn isDeleted(self: *const ObjectHeader) bool {
        return (self.flags & 0x01) != 0;
    }
    
    pub fn setDeleted(self: *ObjectHeader, deleted: bool) void {
        if (deleted) {
            self.flags |= 0x01;
        } else {
            self.flags &= ~@as(u8, 0x01);
        }
    }
};
```

**Key Design Decisions**:
- `extern struct` for predictable layout
- Fits in 64-byte cache line
- Bit flags for efficient storage
- No padding waste

### 3. Object - Complete Structure

**Purpose**: Full object with header, data, and references.

**Zig Implementation**:
```zig
pub const Object = struct {
    header: ObjectHeader,
    fixed: []u8,        // Fixed-size fields
    var_data: []u8,     // Variable data (CBOR)
    refs: []ObjRef,     // References to other objects
    allocator: Allocator,

    pub fn init(allocator: Allocator, type_tag: u16, id: [16]u8) !Object {
        return .{
            .header = ObjectHeader.init(type_tag, id, 0),
            .fixed = &[_]u8{},
            .var_data = &[_]u8{},
            .refs = &[_]ObjRef{},
            .allocator = allocator,
        };
    }

    pub fn deinit(self: *Object) void {
        if (self.fixed.len > 0) self.allocator.free(self.fixed);
        if (self.var_data.len > 0) self.allocator.free(self.var_data);
        if (self.refs.len > 0) self.allocator.free(self.refs);
    }

    // Get reference by field index (for navigation)
    pub fn getReference(self: *const Object, index: u32) ?ObjRef {
        if (index >= self.refs.len) return null;
        return self.refs[index];
    }

    // Serialize to bytes (for storage)
    pub fn toBytes(self: *const Object, buf: []u8) !usize {
        var offset: usize = 0;

        // Header
        const header_bytes = std.mem.asBytes(&self.header);
        @memcpy(buf[offset..][0..header_bytes.len], header_bytes);
        offset += header_bytes.len;

        // Lengths (3 x u32)
        std.mem.writeInt(u32, buf[offset..][0..4], @intCast(self.fixed.len), .little);
        offset += 4;
        std.mem.writeInt(u32, buf[offset..][0..4], @intCast(self.var_data.len), .little);
        offset += 4;
        std.mem.writeInt(u32, buf[offset..][0..4], @intCast(self.refs.len), .little);
        offset += 4;

        // Data
        @memcpy(buf[offset..][0..self.fixed.len], self.fixed);
        offset += self.fixed.len;
        @memcpy(buf[offset..][0..self.var_data.len], self.var_data);
        offset += self.var_data.len;

        // References
        for (self.refs) |ref| {
            std.mem.writeInt(u64, buf[offset..][0..8], ref.offset, .little);
            offset += 8;
        }

        return offset;
    }
};
```

**Key Design Decisions**:
- Explicit allocator parameter (no hidden allocations)
- Slices instead of Vec (no capacity overhead)
- Zero-copy serialization where possible
- Direct memory access for navigation

### 4. Arena - Memory-Mapped Allocator

**Purpose**: Fast allocation with co-location for cache efficiency.

**Zig Implementation**:


```zig
pub const Arena = struct {
    mmap: []align(std.mem.page_size) u8,
    free_list: std.ArrayList(u32),
    bump: std.atomic.Value(u32),
    size: usize,
    used: std.atomic.Value(usize),
    object_count: std.atomic.Value(u64),
    page_usage: []std.atomic.Value(u32),  // Track usage per 4KB page
    allocator: Allocator,

    pub fn init(allocator: Allocator, path: []const u8, size: usize) !Arena {
        // Open/create file
        const file = try std.fs.cwd().createFile(path, .{ .read = true, .truncate = false });
        defer file.close();

        // Set file size
        try file.setEndPos(size);

        // Memory map with huge page hint
        const mmap = try std.posix.mmap(
            null,
            size,
            std.posix.PROT.READ | std.posix.PROT.WRITE,
            .{ .TYPE = .SHARED },
            file.handle,
            0,
        );

        // Advise kernel about access pattern
        try std.posix.madvise(mmap.ptr, size, std.posix.MADV.RANDOM);

        // Allocate page tracking
        const num_pages = (size + 4095) / 4096;
        const page_usage = try allocator.alloc(std.atomic.Value(u32), num_pages);
        for (page_usage) |*p| {
            p.* = std.atomic.Value(u32).init(0);
        }

        return .{
            .mmap = mmap,
            .free_list = std.ArrayList(u32).init(allocator),
            .bump = std.atomic.Value(u32).init(64), // Skip offset 0 (null)
            .size = size,
            .used = std.atomic.Value(usize).init(0),
            .object_count = std.atomic.Value(u64).init(0),
            .page_usage = page_usage,
            .allocator = allocator,
        };
    }

    pub fn deinit(self: *Arena) void {
        std.posix.munmap(self.mmap);
        self.free_list.deinit();
        self.allocator.free(self.page_usage);
    }

    // Fast bump allocation
    pub fn alloc(self: *Arena, size: u32) !ObjRef {
        // Try free list for small allocations
        if (size <= 64 and self.free_list.popOrNull()) |slot| {
            _ = self.object_count.fetchAdd(1, .seq_cst);
            return ObjRef.init(@as(u64, slot) * 64);
        }

        // Bump allocate
        const offset = self.bump.fetchAdd(size, .seq_cst);
        if (offset + size > self.size) return error.OutOfMemory;

        _ = self.used.fetchAdd(size, .seq_cst);
        _ = self.object_count.fetchAdd(1, .seq_cst);

        return ObjRef.init(offset);
    }

    // Co-located allocation (critical for O(1) navigation)
    pub fn allocNear(self: *Arena, size: u32, near: ObjRef) !ObjRef {
        const page_id = near.pageId();
        
        // Try to allocate in same page
        if (self.allocInPage(page_id, size)) |offset| {
            _ = self.object_count.fetchAdd(1, .seq_cst);
            return ObjRef.init(offset);
        }
        
        // Try adjacent pages
        if (page_id > 0) {
            if (self.allocInPage(page_id - 1, size)) |offset| {
                _ = self.object_count.fetchAdd(1, .seq_cst);
                return ObjRef.init(offset);
            }
        }
        if (self.allocInPage(page_id + 1, size)) |offset| {
            _ = self.object_count.fetchAdd(1, .seq_cst);
            return ObjRef.init(offset);
        }
        
        // Fall back to regular allocation
        return self.alloc(size);
    }

    fn allocInPage(self: *Arena, page_id: u32, size: u32) ?u64 {
        if (page_id >= self.page_usage.len) return null;
        
        const page_start = @as(u64, page_id) * 4096;
        const current_usage = self.page_usage[page_id].load(.seq_cst);
        
        // Check if space available in page
        if (current_usage + size <= 4096) {
            const offset = page_start + current_usage;
            
            // Atomic update of page usage
            _ = self.page_usage[page_id].fetchAdd(size, .seq_cst);
            _ = self.used.fetchAdd(size, .seq_cst);
            
            // Update bump pointer if needed
            const current_bump = self.bump.load(.seq_cst);
            const new_bump = @max(current_bump, @as(u32, @intCast(offset + size)));
            _ = self.bump.cmpxchgWeak(current_bump, new_bump, .seq_cst, .seq_cst);
            
            return offset;
        }
        
        return null;
    }

    // Write object to arena
    pub fn write(self: *Arena, ref: ObjRef, data: []const u8) !void {
        if (ref.isNull()) return error.InvalidReference;
        if (ref.offset + data.len > self.size) return error.OutOfBounds;

        const dest = self.mmap[ref.offset..][0..data.len];
        @memcpy(dest, data);
    }

    // Read object from arena (zero-copy)
    pub fn read(self: *Arena, ref: ObjRef) ![]const u8 {
        if (ref.isNull()) return error.InvalidReference;

        // Read header to get size
        const header_size = @sizeOf(ObjectHeader);
        if (ref.offset + header_size > self.size) return error.OutOfBounds;

        const header_bytes = self.mmap[ref.offset..][0..header_size];
        const header = std.mem.bytesAsValue(ObjectHeader, header_bytes);

        if (ref.offset + header.size > self.size) return error.OutOfBounds;

        return self.mmap[ref.offset..][0..header.size];
    }
};
```

**Key Design Decisions**:
- Direct `std.posix.mmap()` - no wrapper overhead
- Atomic operations for thread safety
- Co-location algorithm prioritizes same page, then adjacent
- Zero-copy reads (return slice into mmap)
- Explicit allocator for page tracking

### 5. WAL - Write-Ahead Log

**Purpose**: Durability and crash recovery.

**Zig Implementation**:
```zig
pub const WALEntry = extern union {
    tag: u8,
    insert: extern struct {
        tag: u8 = 1,
        lsn: u64,
        objref: ObjRef,
        data_len: u32,
        // data follows in separate buffer
    },
    update: extern struct {
        tag: u8 = 2,
        lsn: u64,
        objref: ObjRef,
        data_len: u32,
    },
    delete: extern struct {
        tag: u8 = 3,
        lsn: u64,
        objref: ObjRef,
    },
    commit: extern struct {
        tag: u8 = 4,
        lsn: u64,
        txn_id: u64,
    },
};

pub const WAL = struct {
    file: std.fs.File,
    buffer: std.ArrayList(u8),
    next_lsn: std.atomic.Value(u64),
    last_sync_lsn: std.atomic.Value(u64),
    allocator: Allocator,

    pub fn init(allocator: Allocator, path: []const u8) !WAL {
        const file = try std.fs.cwd().createFile(path, .{ .read = true, .truncate = false });
        
        return .{
            .file = file,
            .buffer = std.ArrayList(u8).init(allocator),
            .next_lsn = std.atomic.Value(u64).init(1),
            .last_sync_lsn = std.atomic.Value(u64).init(0),
            .allocator = allocator,
        };
    }

    pub fn append(self: *WAL, entry: WALEntry, data: ?[]const u8) !u64 {
        const lsn = self.next_lsn.fetchAdd(1, .seq_cst);
        
        // Write entry
        const entry_bytes = std.mem.asBytes(&entry);
        try self.buffer.appendSlice(entry_bytes);
        
        // Write data if present
        if (data) |d| {
            try self.buffer.appendSlice(d);
        }
        
        // Group commit: flush if buffer full
        if (self.buffer.items.len >= 16 * 1024) {
            try self.sync();
        }
        
        return lsn;
    }

    pub fn sync(self: *WAL) !void {
        try self.file.writeAll(self.buffer.items);
        try self.file.sync();
        self.buffer.clearRetainingCapacity();
        self.last_sync_lsn.store(self.next_lsn.load(.seq_cst), .seq_cst);
    }
};
```

**Key Design Decisions**:
- `extern union` for C-compatible tagged union
- Buffered writes with group commit
- Atomic LSN for thread safety
- Explicit sync control

## C API Design

### Core Operations

```zig
// Arena management
export fn owl_arena_create(path: [*:0]const u8, size: usize) ?*Arena;
export fn owl_arena_destroy(arena: *Arena) void;

// Allocation (hot path - must be fast!)
export fn owl_arena_alloc(arena: *Arena, size: u32) ?*ObjRef;
export fn owl_arena_alloc_near(arena: *Arena, size: u32, near: *const ObjRef) ?*ObjRef;

// I/O operations
export fn owl_arena_write(arena: *Arena, ref: *const ObjRef, data: [*]const u8, len: usize) i32;
export fn owl_arena_read(arena: *Arena, ref: *const ObjRef, buf: [*]u8, len: usize) isize;
export fn owl_arena_delete(arena: *Arena, ref: *const ObjRef) i32;

// Navigation (critical path!)
export fn owl_object_get_reference(arena: *Arena, ref: *const ObjRef, field_index: u32) ?*ObjRef;
export fn owl_object_get_field(arena: *Arena, ref: *const ObjRef, field_index: u32, buf: [*]u8, len: usize) isize;

// Statistics
export fn owl_arena_stats(arena: *const Arena, stats: *Stats) void;
```

### Rust Wrapper

```rust
pub struct Arena {
    inner: *mut c_void,
}

impl Arena {
    pub fn new(path: &Path, size: usize) -> Result<Self> {
        let path_cstr = CString::new(path.to_str().unwrap())?;
        let inner = unsafe { owl_arena_create(path_cstr.as_ptr(), size) };
        if inner.is_null() {
            return Err(Error::AllocationFailed);
        }
        Ok(Self { inner })
    }

    // Hot path - inline for performance
    #[inline]
    pub fn get_reference(&self, obj_ref: &ObjRef, field: u32) -> Option<ObjRef> {
        unsafe {
            let result = owl_object_get_reference(self.inner, obj_ref as *const _, field);
            result.as_ref().copied()
        }
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        unsafe { owl_arena_destroy(self.inner) };
    }
}
```

**Key Design Decisions**:
- Minimal C API surface
- Hot path functions are simple (no complex types)
- RAII in Rust for cleanup
- Inline wrappers for zero overhead

## Performance Optimizations

### 1. Co-Location Strategy

**Goal**: Keep related objects in same cache line/page.

**Algorithm**:
```
When allocating object B that references object A:
1. Get page_id of A
2. Try to allocate B in same page
3. If no space, try adjacent pages (±1)
4. If still no space, use bump allocator
5. Track co-location success rate
```

**Expected Results**:
- 70%+ co-location success rate
- Navigation stays in L1/L2 cache
- <50ns per navigation hop

### 2. Batch Swizzling

**Goal**: Amortize page load cost across multiple objects.

**Algorithm**:
```
When loading multiple objects:
1. Group ObjRefs by page_id
2. Prefetch all pages (madvise WILLNEED)
3. Swizzle all refs in batch
4. Return array of pointers
```

**Expected Results**:
- 5-10x faster than individual loads
- Better cache utilization
- Reduced system calls

### 3. Memory Layout

**Goal**: Minimize cache misses.

**Strategy**:
- ObjectHeader fits in 64-byte cache line
- Fixed fields aligned to natural boundaries
- References stored contiguously
- Hot fields at start of object

## Testing Strategy

### Unit Tests (Zig)

```zig
test "ObjRef swizzling" {
    const ref = ObjRef.init(4096 + 128);
    const base: [*]u8 = @ptrFromInt(0x1000000);
    const ptr = ref.swizzle(base);
    try testing.expectEqual(@intFromPtr(ptr), 0x1000000 + 4096 + 128);
}

test "Arena allocation" {
    var arena = try Arena.init(testing.allocator, "/tmp/test.dat", 1024 * 1024);
    defer arena.deinit();

    const ref1 = try arena.alloc(128);
    const ref2 = try arena.alloc(256);
    
    try testing.expect(!ref1.isNull());
    try testing.expect(ref2.offset > ref1.offset);
}

test "Co-location" {
    var arena = try Arena.init(testing.allocator, "/tmp/test.dat", 1024 * 1024);
    defer arena.deinit();

    const parent = try arena.alloc(128);
    const child = try arena.allocNear(128, parent);
    
    try testing.expectEqual(parent.pageId(), child.pageId());
}
```

### Integration Tests (Rust)

```rust
#[test]
fn test_object_graph_navigation() {
    let arena = Arena::new("test.dat", 10_000_000).unwrap();
    
    // Create: Order -> Customer -> Profile
    let profile_ref = arena.alloc(128, None).unwrap();
    let customer_ref = arena.alloc(256, Some(profile_ref)).unwrap();
    let order_ref = arena.alloc(512, Some(customer_ref)).unwrap();
    
    // Navigate: order.customer.profile
    let customer = arena.get_reference(&order_ref, 0).unwrap();
    let profile = arena.get_reference(&customer, 0).unwrap();
    
    assert_eq!(profile, profile_ref);
}
```

## Migration Strategy

### Phase 1: Core Types (Week 1-2)
- Implement ObjRef, ObjectHeader, Object
- C API exports
- Rust bindings
- Unit tests

### Phase 2: Arena (Week 3-4)
- Implement Arena with mmap
- Co-location algorithm
- Allocation tests
- Benchmark vs Rust

### Phase 3: WAL (Week 5-6)
- Implement WAL structure
- Append/sync operations
- Recovery logic
- Crash tests

### Phase 4: Integration (Week 7-8)
- Connect Zig storage to Rust query layer
- Port all tests
- Performance tuning
- Documentation

**Total**: 8 weeks for core storage engine

## Success Criteria

### Performance
- ✅ Navigation: <50ns per hop (vs 125ns Rust)
- ✅ Allocation: <20ns (vs 50-100ns Rust)
- ✅ Memory: <1.2x overhead (vs 1.5-2x Rust)
- ✅ Compile: <10s (vs 60s+ Rust)

### Correctness
- ✅ All tests pass
- ✅ No memory leaks (valgrind clean)
- ✅ No data races (ThreadSanitizer clean)
- ✅ Crash recovery works

### Integration
- ✅ Clean C API
- ✅ Ergonomic Rust wrappers
- ✅ Zero-copy where possible
- ✅ Good error handling

## Key Insights

1. **OQL is about navigation, not queries** - Zig's pointer arithmetic is perfect
2. **Co-location is critical** - Zig's manual memory control enables this
3. **Zero-copy is essential** - Zig's slices and mmap integration shine here
4. **C ABI is simple** - Just pointers and integers, no complex types
5. **Performance compounds** - Navigation runs millions of times per query

This design leverages Zig's strengths (manual control, zero-cost abstractions, C interop) while keeping Rust's strengths (safety, ecosystem) for the query layer.
