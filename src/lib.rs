pub mod app;
pub mod capture;
pub mod session;
pub mod wire;

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

// mi_collect only purges the calling thread's own heap (mimalloc has no process-wide collect),
// so this must run on whichever thread actually dropped the freed memory, not on a background
// task's thread.
pub fn trim_heap() {
    unsafe { libmimalloc_sys::mi_collect(true) };
}
