#![feature(lang_items, needs_panic_runtime, asm, libc, core_slice_ext)]

#![no_std]
#![needs_panic_runtime]

extern crate libc;

#[path = "../src/board.rs"]
mod board;
#[path = "../src/mailbox.rs"]
mod mailbox;
#[path = "../src/kernel_proto.rs"]
mod kernel_proto;

mod dyld;
mod api;

use core::{mem, ptr, slice, str};
use libc::{c_char, size_t};
use kernel_proto::*;
use dyld::Library;

fn send(request: &Message) {
    unsafe { mailbox::send(request as *const _ as usize) }
    while !mailbox::acknowledged() {}
}

fn recv<R, F: FnOnce(&Message) -> R>(f: F) -> R {
    while mailbox::receive() == 0 {}
    let result = f(unsafe { mem::transmute::<usize, &Message>(mailbox::receive()) });
    mailbox::acknowledge();
    result
}

macro_rules! recv {
    ($p:pat => $e:expr) => {
        recv(|request| {
            if let $p = request {
                $e
            } else {
                send(&Log(format_args!("unexpected reply: {:?}", request)));
                loop {}
            }
        })
    }
}

macro_rules! print {
    ($($arg:tt)*) => ($crate::send(&Log(format_args!($($arg)*))));
}

macro_rules! println {
    ($fmt:expr) => (print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => (print!(concat!($fmt, "\n"), $($arg)*));
}

#[lang = "panic_fmt"]
extern fn panic_fmt(args: core::fmt::Arguments, file: &'static str, line: u32) -> ! {
    println!("panic at {}:{}: {}", file, line, args);
    send(&RunAborted);
    loop {}
}

static mut NOW: u64 = 0;

#[no_mangle]
pub extern fn send_to_log(ptr: *const u8, len: usize) {
    send(&LogSlice(unsafe {
        str::from_utf8_unchecked(slice::from_raw_parts(ptr, len))
    }))
}

extern fn abort() -> ! {
    println!("kernel called abort()");
    send(&RunAborted);
    loop {}
}

extern fn send_rpc(service: u32, tag: *const u8, data: *const *const ()) {
    extern { fn strlen(s: *const c_char) -> size_t; }
    let tag = unsafe { slice::from_raw_parts(tag, strlen(tag as *const c_char)) };

    send(&RpcSend {
        service: service as u32,
        batch:   service == 0,
        tag:     tag,
        data:    data
    })
}

extern fn recv_rpc(slot: *mut ()) -> usize {
    send(&RpcRecvRequest(slot));
    recv!(&RpcRecvReply(ref result) => {
        match result {
            &Ok(alloc_size) => alloc_size,
            &Err(ref exception) => unsafe { __artiq_raise(exception as *const _) }
        }
    })
}

#[allow(improper_ctypes)]
extern {
    fn __artiq_raise(exn: *const ::kernel_proto::Exception) -> !;
}

macro_rules! artiq_raise {
    ($name:expr, $message:expr) => ({
        let exn = Exception {
            name:     concat!("0:artiq.coredevice.exceptions.", $name, "\0").as_bytes().as_ptr(),
            file:     concat!(file!(), "\0").as_bytes().as_ptr(),
            line:     line!(),
            column:   column!(),
            // https://github.com/rust-lang/rfcs/pull/1719
            function: "(Rust function)\0".as_bytes().as_ptr(),
            message:  concat!($message, "\0").as_bytes().as_ptr(),
            param:    [0; 3],
            phantom:  ::core::marker::PhantomData
        };
        unsafe { __artiq_raise(&exn as *const _) }
    })
}

#[no_mangle]
pub extern fn __artiq_terminate(exception: *const kernel_proto::Exception,
                            backtrace_data: *mut usize,
                            backtrace_size: usize) -> ! {
    let backtrace = unsafe { slice::from_raw_parts_mut(backtrace_data, backtrace_size) };
    let mut cursor = 0;
    for index in 0..backtrace.len() {
        if backtrace[index] > kernel_proto::KERNELCPU_PAYLOAD_ADDRESS {
            backtrace[cursor] = backtrace[index] - kernel_proto::KERNELCPU_PAYLOAD_ADDRESS;
            cursor += 1;
        }
    }
    let backtrace = &mut backtrace[0..cursor];

    send(&NowSave(unsafe { NOW }));
    send(&RunException {
        exception: unsafe { (*exception).clone() },
        backtrace: backtrace
    });
    loop {}
}

extern fn watchdog_set(ms: i64) -> usize {
    if ms < 0 {
        artiq_raise!("ValueError", "cannot set a watchdog with a negative timeout")
    }

    send(&WatchdogSetRequest { ms: ms as u64 });
    recv!(&WatchdogSetReply { id } => id)
}

extern fn watchdog_clear(id: usize) {
    send(&WatchdogClear { id: id })
}

extern fn cache_get(key: *const u8) -> (usize, *const u32) {
    extern { fn strlen(s: *const c_char) -> size_t; }
    let key = unsafe { slice::from_raw_parts(key, strlen(key as *const c_char)) };
    let key = unsafe { str::from_utf8_unchecked(key) };

    send(&CacheGetRequest { key: key });
    recv!(&CacheGetReply { value } => (value.len(), value.as_ptr()))
}

extern fn cache_put(key: *const u8, &(len, ptr): &(usize, *const u32)) {
    extern { fn strlen(s: *const c_char) -> size_t; }
    let key = unsafe { slice::from_raw_parts(key, strlen(key as *const c_char)) };
    let key = unsafe { str::from_utf8_unchecked(key) };

    let value = unsafe { slice::from_raw_parts(ptr, len) };
    send(&CachePutRequest { key: key, value: value });
    recv!(&CachePutReply { succeeded } => {
        if !succeeded {
            artiq_raise!("CacheError", "cannot put into a busy cache row")
        }
    })
}

unsafe fn attribute_writeback(typeinfo: *const ()) {
    struct Attr {
        offset: usize,
        tag:    *const u8,
        name:   *const u8
    }

    struct Type {
        attributes: *const *const Attr,
        objects:    *const *const ()
    }

    let mut tys = typeinfo as *const *const Type;
    while !(*tys).is_null() {
        let ty = *tys;
        tys = tys.offset(1);

        let mut objects = (*ty).objects;
        while !(*objects).is_null() {
            let object = *objects;
            objects = objects.offset(1);

            let mut attributes = (*ty).attributes;
            while !(*attributes).is_null() {
                let attribute = *attributes;
                attributes = attributes.offset(1);

                if !(*attribute).tag.is_null() {
                    send_rpc(0, (*attribute).tag, [
                        &object as *const _ as *const (),
                        &(*attribute).name as *const _ as *const (),
                        (object as usize + (*attribute).offset) as *const ()
                    ].as_ptr());
                }
            }
        }
    }
}

#[no_mangle]
pub unsafe fn main() {
    let library = recv!(&LoadRequest(library) => {
        match Library::load(library, kernel_proto::KERNELCPU_PAYLOAD_ADDRESS, api::resolve) {
            Err(error) => {
                send(&LoadReply(Err(error)));
                loop {}
            },
            Ok(library) => {
                send(&LoadReply(Ok(())));
                library
            }
        }
    });

    let __bss_start = library.lookup("__bss_start");
    let _end = library.lookup("_end");
    ptr::write_bytes(__bss_start as *mut u8, 0, _end - __bss_start);

    send(&NowInitRequest);
    recv!(&NowInitReply(now) => NOW = now);
    (mem::transmute::<usize, fn()>(library.lookup("__modinit__")))();
    send(&NowSave(NOW));

    attribute_writeback(library.lookup("typeinfo") as *const ());

    send(&RunFinished);

    loop {}
}

#[no_mangle]
pub fn exception_handler(vect: u32, _regs: *const u32, pc: u32, ea: u32) {
    println!("exception {:?} at PC 0x{:x}, EA 0x{:x}", vect, pc, ea);
    send(&RunAborted)
}
