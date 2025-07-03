//! Thread Local Storage (TLS) support.
//!
//! ## TLS layout for x86_64
//!
//! ```text
//! aligned --> +-------------------------+- static_tls_offset
//! allocation  |                         | \
//!             | .tdata                  |  |
//! | address   |                         |  |
//! | grow up   + - - - - - - - - - - - - +   > Static TLS block
//! v           |                         |  |  (length: static_tls_size)
//!             | .tbss                   |  |
//!             |                         |  |
//!             +-------------------------+  |
//!             | / PADDING / / / / / / / | /
//!             +-------------------------+
//!    tls_ptr -+-> self pointer (void *) | \
//! (tp_offset) |                         |  |
//!             | Custom TCB format       |   > Thread Control Block (TCB)
//!             | (might be used          |  |  (length: TCB_SIZE)
//!             |  by a libC)             |  |
//!             |                         | /
//!             +-------------------------+- (total length: tls_area_size)
//! ```
//!
//! ## TLS layout for AArch64 and RISC-V
//!
//! ```text
//!             +-------------------------+
//!             |                         | \
//!             | Custom TCB format       |  |
//!             | (might be used          |   > Thread Control Block (TCB)
//!             |  by a libC)             |  |  (length: TCB_SIZE)
//!             |                         | /
//!    tls_ptr -+-------------------------+
//! (tp_offset) | GAP_ABOVE_TP            |
//!             +-------------------------+- static_tls_offset
//!             |                         | \
//!             | .tdata                  |  |
//!             |                         |  |
//!             + - - - - - - - - - - - - +   > Static TLS block
//!             |                         |  |  (length: static_tls_size)
//!             | .tbss                   |  |
//!             |                         | /
//!             +-------------------------+- (total length: tls_area_size)
//! ```
//!
//! Reference:
//! 1. <https://github.com/unikraft/unikraft/blob/staging/arch/x86/x86_64/tls.c>
//! 2. <https://github.com/unikraft/unikraft/blob/staging/arch/arm/arm64/tls.c>

use memory_addr::align_up;

use core::alloc::Layout;
use core::ptr::NonNull;

const TLS_ALIGN: usize = 0x10;

cfg_if::cfg_if! {
    if #[cfg(target_arch = "x86_64")] {
        const TCB_SIZE: usize = 8; // to store TLS self pointer
        const GAP_ABOVE_TP: usize = 0;
    } else if #[cfg(target_arch = "aarch64")] {
        const TCB_SIZE: usize = 0;
        const GAP_ABOVE_TP: usize = 16;
    } else if #[cfg(target_arch = "riscv64")] {
        const TCB_SIZE: usize = 0;
        const GAP_ABOVE_TP: usize = 0;
    }
}

/// The memory region for thread-local storage.
pub struct TlsArea {
    base: NonNull<u8>,
    layout: Layout,
    tls_ptr: usize,
}

impl TlsArea {
    /// Returns the pointer to the TLS static area.
    ///
    /// One should set the hardware thread pointer register to this value.
    pub fn tls_ptr(&self) -> *mut u8 {
        self.tls_ptr as _
    }
}
