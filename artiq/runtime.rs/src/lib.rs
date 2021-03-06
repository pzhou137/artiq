#![no_std]
#![feature(libc, const_fn, try_borrow, stmt_expr_attributes, repr_simd, asm)]

#[macro_use]
extern crate std_artiq as std;
extern crate libc;
#[macro_use]
extern crate log;
extern crate log_buffer;
extern crate byteorder;
extern crate fringe;
extern crate lwip;

use logger::BufferLogger;

mod board;
mod config;
mod clock;
mod rtio_crg;
mod mailbox;

mod urc;
mod sched;
mod logger;
mod cache;

mod proto;
mod kernel_proto;
mod session_proto;
mod moninj_proto;
mod analyzer_proto;

mod kernel;
mod rpc;
mod session;
mod moninj;
#[cfg(has_rtio_analyzer)]
mod analyzer;

extern {
    fn network_init();
    fn lwip_service();
}

include!(concat!(env!("OUT_DIR"), "/git_info.rs"));

#[no_mangle]
pub unsafe extern fn rust_main() {
    static mut LOG_BUFFER: [u8; 4096] = [0; 4096];
    BufferLogger::new(&mut LOG_BUFFER[..])
                 .register(move || {
        info!("booting ARTIQ...");
        info!("software version {}", GIT_COMMIT);
        info!("gateware version {}", ::board::ident(&mut [0; 64]));

        clock::init();
        rtio_crg::init();
        network_init();

        let mut scheduler = sched::Scheduler::new();
        scheduler.spawner().spawn(16384, session::thread);
        scheduler.spawner().spawn(4096, moninj::thread);
        #[cfg(has_rtio_analyzer)]
        scheduler.spawner().spawn(4096, analyzer::thread);

        loop {
            scheduler.run();
            lwip_service();
        }
    })
}

#[no_mangle]
pub unsafe extern fn isr() {
    use board::{irq, csr};
    extern { fn uart_isr(); }

    let irqs = irq::pending() & irq::get_mask();
    if irqs & (1 << csr::UART_INTERRUPT) != 0 {
        uart_isr()
    }
}

#[no_mangle]
pub fn sys_now() -> u32 {
    clock::get_ms() as u32
}

#[no_mangle]
pub fn sys_jiffies() -> u32 {
    clock::get_ms() as u32
}
