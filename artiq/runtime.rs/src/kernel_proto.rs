#![allow(dead_code)]

use core::marker::PhantomData;
use core::fmt;

pub const KERNELCPU_EXEC_ADDRESS:    usize = 0x40400000;
pub const KERNELCPU_PAYLOAD_ADDRESS: usize = 0x40440000;
pub const KERNELCPU_LAST_ADDRESS:    usize = 0x4fffffff;
pub const KSUPPORT_HEADER_SIZE:      usize = 0x80;

#[repr(C)]
#[derive(Debug, Clone)]
pub struct Exception<'a> {
    pub name:     *const u8,
    pub file:     *const u8,
    pub line:     u32,
    pub column:   u32,
    pub function: *const u8,
    pub message:  *const u8,
    pub param:    [u64; 3],
    pub phantom:  PhantomData<&'a str>
}

#[derive(Debug)]
pub enum Message<'a> {
    LoadRequest(&'a [u8]),
    LoadReply(Result<(), &'a str>),

    NowInitRequest,
    NowInitReply(u64),
    NowSave(u64),

    RunFinished,
    RunException {
        exception: Exception<'a>,
        backtrace: &'a [usize]
    },
    RunAborted,

    WatchdogSetRequest { ms: u64 },
    WatchdogSetReply   { id: usize },
    WatchdogClear      { id: usize },

    RpcSend {
        service: u32,
        batch: bool,
        tag: &'a [u8],
        data: *const *const ()
    },
    RpcRecvRequest(*mut ()),
    RpcRecvReply(Result<usize, Exception<'a>>),

    CacheGetRequest { key: &'a str },
    CacheGetReply   { value: &'static [u32] },
    CachePutRequest { key: &'a str, value: &'static [u32] },
    CachePutReply   { succeeded: bool },

    Log(fmt::Arguments<'a>),
    LogSlice(&'a str)
}

pub use self::Message::*;
