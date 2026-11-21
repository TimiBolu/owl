
export function owl_read(handle, buffer, at) { return handle.read(buffer, { at }); }
export function owl_write(handle, buffer, at) { return handle.write(buffer, { at }); }
export function owl_truncate(handle, length) { handle.truncate(length); }
export function owl_size(handle) { return handle.getSize(); }
export function owl_flush(handle) { handle.flush(); }
