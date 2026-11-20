//! Machine-specific code. `x86_64` is the only machine today; keeping PCI,
//! ATA, USB and PS/2 here is the seam that stops PC port I/O from spreading
//! through the rest of the kernel (ADR 0052).

pub mod x86_64;
