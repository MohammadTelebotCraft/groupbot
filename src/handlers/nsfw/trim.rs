#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub fn release_freed() {
    unsafe extern "C" {
        fn malloc_trim(pad: usize) -> i32;
    }
    unsafe {
        malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn release_freed() {}
