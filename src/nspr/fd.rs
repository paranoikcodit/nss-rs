/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use libc::c_void;
use nspr::bool_from_nspr;
use nspr::error::{Result, PR_ADDRESS_NOT_SUPPORTED_ERROR};
use nspr::net::{read_net_addr, write_net_addr, NetAddrStorage};
use nspr::time::duration_opt_to_nspr;
use nss_sys::nspr as ffi;
use std::ffi::CString;
use std::i32;
use std::marker::PhantomData;
use std::mem;
use std::net::SocketAddr;
use std::ops::{Deref, DerefMut};
use std::ptr;
use std::sync::Arc;
use std::time::Duration;
use {wrap_ffi, GenStatus};

pub type RawFile = *mut ffi::PRFileDesc;

pub struct File(RawFile);
unsafe impl Sync for File {}
unsafe impl Send for File {}

impl Drop for File {
    fn drop(&mut self) {
        let fd = mem::replace(&mut self.0, null());
        if fd != null() {
            let _status: Result<()> = wrap_ffi(|| unsafe { ffi::PR_Close(fd) });
        }
    }
}

#[allow(dead_code)]
impl File {
    pub unsafe fn unsafe_get_ref<Inner>(&self) -> &Inner {
        let ptr: *mut ffi::PRFilePrivate = (*self.as_raw_prfd()).secret;
        mem::transmute(ptr)
    }
    pub unsafe fn unsafe_get_mut<Inner>(&mut self) -> &mut Inner {
        let ptr: *mut ffi::PRFilePrivate = (*self.as_raw_prfd()).secret;
        mem::transmute(ptr)
    }

    pub fn into_raw_prfd(self) -> RawFile {
        let fd = self.as_raw_prfd();
        mem::forget(self);
        fd
    }
    pub fn as_raw_prfd(&self) -> RawFile {
        debug_assert!(self.0 != null());
        self.0
    }
    pub unsafe fn from_raw_prfd(fd: RawFile) -> Self {
        assert!(fd != null());
        File(fd)
    }
    pub unsafe fn from_raw_prfd_opt(fd: RawFile) -> Option<Self> {
        if fd == null() {
            None
        } else {
            Some(Self::from_raw_prfd(fd))
        }
    }
    pub unsafe fn from_raw_prfd_err(fd: RawFile) -> GenStatus<Self> {
        if fd == null() {
            GenStatus::ErrorFromC
        } else {
            GenStatus::Success(Self::from_raw_prfd(fd))
        }
    }
}

// Like `File`, but with no `drop`; for use in callbacks from C where
// the caller owns the file and the callee must not close it.  In
// general this should be used only via `&File` borrows.
pub struct BorrowedFile<Secret = ()>(RawFile, PhantomData<Secret>);
// This could get the same `unsafe impl`s as owned `File`, but
// probably doesn't need them.

impl<Secret> Deref for BorrowedFile<Secret> {
    type Target = File;
    fn deref(&self) -> &File {
        unsafe { mem::transmute(self) }
    }
}
impl<Secret> DerefMut for BorrowedFile<Secret> {
    fn deref_mut(&mut self) -> &mut File {
        unsafe { mem::transmute(self) }
    }
}

impl<Secret> BorrowedFile<Secret> {
    pub unsafe fn from_raw_prfd(fd: RawFile) -> Self {
        assert!(fd != null());
        BorrowedFile(fd, PhantomData)
    }
    pub unsafe fn from_raw_prfd_checked(fd: RawFile, ident: ffi::PRDescIdentity) -> Self {
        assert_eq!((*fd).identity, ident);
        Self::from_raw_prfd(fd)
    }
    pub fn get_ref(&self) -> &Secret {
        unsafe { self.unsafe_get_ref() }
    }
    pub fn get_mut(&mut self) -> &mut Secret {
        unsafe { self.unsafe_get_mut() }
    }
}

pub fn new_pipe() -> Result<(File, File)> {
    super::init();
    let mut reader = null();
    let mut writer = null();
    unsafe {
        wrap_ffi(|| ffi::PR_CreatePipe(&mut reader, &mut writer))?;
        Ok((File::from_raw_prfd(reader), File::from_raw_prfd(writer)))
    }
}

pub fn null() -> RawFile {
    ptr::null_mut()
}

pub trait FileMethods {
    fn read(&self, _buf: &mut [u8]) -> Result<usize> {
        unimplemented!()
    }
    fn write(&self, _buf: &[u8]) -> Result<usize> {
        unimplemented!()
    }
    fn connect(&self, _addr: SocketAddr, _timeout: Option<Duration>) -> Result<()> {
        unimplemented!()
    }
    // FIXME: use a special enum for peek, not bool.
    fn recv(&self, _buf: &mut [u8], _peek: bool, _timeout: Option<Duration>) -> Result<usize> {
        unimplemented!()
    }
    fn send(&self, _buf: &[u8], _timeout: Option<Duration>) -> Result<usize> {
        unimplemented!()
    }
    fn getsockname(&self) -> Result<SocketAddr> {
        unimplemented!()
    }
    fn getpeername(&self) -> Result<SocketAddr> {
        unimplemented!()
    }
    fn get_nonblocking(&self) -> Result<bool> {
        unimplemented!()
    }
}

impl FileMethods for File {
    fn read(&self, buf: &mut [u8]) -> Result<usize> {
        assert!(buf.len() <= i32::MAX as usize);
        wrap_ffi(|| unsafe {
            ffi::PR_Read(
                self.as_raw_prfd(),
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as i32,
            )
        })
    }

    fn write(&self, buf: &[u8]) -> Result<usize> {
        assert!(buf.len() <= i32::MAX as usize);
        wrap_ffi(|| unsafe {
            ffi::PR_Write(
                self.as_raw_prfd(),
                buf.as_ptr() as *const c_void,
                buf.len() as i32,
            )
        })
    }

    fn connect(&self, addr: SocketAddr, timeout: Option<Duration>) -> Result<()> {
        let mut addrbuf = NetAddrStorage::new();
        wrap_ffi(|| unsafe {
            write_net_addr(addrbuf.as_mut_ptr(), addr);
            ffi::PR_Connect(
                self.as_raw_prfd(),
                addrbuf.as_ptr(),
                duration_opt_to_nspr(timeout),
            )
        })
    }

    fn recv(&self, buf: &mut [u8], peek: bool, timeout: Option<Duration>) -> Result<usize> {
        assert!(buf.len() <= i32::MAX as usize);
        let flags = if peek { ffi::PR_MSG_PEEK } else { 0 };
        wrap_ffi(|| unsafe {
            ffi::PR_Recv(
                self.as_raw_prfd(),
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as i32,
                flags,
                duration_opt_to_nspr(timeout),
            )
        })
    }

    fn send(&self, buf: &[u8], timeout: Option<Duration>) -> Result<usize> {
        assert!(buf.len() <= i32::MAX as usize);
        let flags = 0;
        wrap_ffi(|| unsafe {
            ffi::PR_Send(
                self.as_raw_prfd(),
                buf.as_ptr() as *const c_void,
                buf.len() as i32,
                flags,
                duration_opt_to_nspr(timeout),
            )
        })
    }

    fn getsockname(&self) -> Result<SocketAddr> {
        let mut buf = NetAddrStorage::new();
        wrap_ffi(|| unsafe { ffi::PR_GetSockName(self.as_raw_prfd(), buf.as_mut_ptr()) })?;
        match unsafe { read_net_addr(buf.as_ptr()) } {
            Some(addr) => Ok(addr),
            None => Err(PR_ADDRESS_NOT_SUPPORTED_ERROR.into()),
        }
    }

    fn getpeername(&self) -> Result<SocketAddr> {
        let mut buf = NetAddrStorage::new();
        wrap_ffi(|| unsafe { ffi::PR_GetPeerName(self.as_raw_prfd(), buf.as_mut_ptr()) })?;
        match unsafe { read_net_addr(buf.as_ptr()) } {
            Some(addr) => Ok(addr),
            None => Err(PR_ADDRESS_NOT_SUPPORTED_ERROR.into()),
        }
    }

    fn get_nonblocking(&self) -> Result<bool> {
        type OptCase = ffi::PRSocketOptionCase<ffi::PRBool>;
        let mut buf = OptCase::new(ffi::PR_SockOpt_Nonblocking, ffi::PR_FALSE);
        wrap_ffi(|| unsafe { ffi::PR_GetSocketOption(self.as_raw_prfd(), buf.as_mut_ptr()) })?;
        Ok(bool_from_nspr(buf.value))
    }
}

pub type FileType = ffi::PRDescType;
pub use nss_sys::nspr::{
    PR_DESC_FILE, PR_DESC_LAYERED, PR_DESC_PIPE, PR_DESC_SOCKET_TCP, PR_DESC_SOCKET_UDP,
};

pub struct FileWrapper<Inner: FileMethods> {
    methods_ref: Arc<ffi::PRIOMethods>,
    phantom: PhantomData<fn(Inner)>,
}

struct WrappedFileImpl<Inner: FileMethods> {
    prfd: ffi::PRFileDesc,
    _methods_ref: Arc<ffi::PRIOMethods>,
    inner: Inner,
}

impl<Inner: FileMethods> Deref for WrappedFileImpl<Inner> {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.inner
    }
}
impl<Inner: FileMethods> DerefMut for WrappedFileImpl<Inner> {
    fn deref_mut(&mut self) -> &mut Inner {
        &mut self.inner
    }
}

impl<Inner: FileMethods> FileWrapper<Inner> {
    pub fn new(file_type: FileType) -> Self {
        let methods = ffi::PRIOMethods {
            file_type: file_type,
            close: Some(wrapper_methods::close::<Inner>),
            read: Some(wrapper_methods::read::<Inner>),
            write: Some(wrapper_methods::write::<Inner>),
            available: None,
            available64: None,
            fsync: None,
            seek: None,
            seek64: None,
            fileInfo: None,
            fileInfo64: None,
            writev: None,
            connect: Some(wrapper_methods::connect::<Inner>),
            accept: None,
            bind: None,
            listen: None,
            shutdown: None,
            recv: Some(wrapper_methods::recv::<Inner>),
            send: Some(wrapper_methods::send::<Inner>),
            recvfrom: None,
            sendto: None,
            poll: None,
            acceptread: None,
            transmitfile: None,
            getsockname: Some(wrapper_methods::getsockname::<Inner>),
            getpeername: Some(wrapper_methods::getpeername::<Inner>),
            reserved_fn_6: None,
            reserved_fn_5: None,
            getsocketoption: Some(wrapper_methods::getsocketoption::<Inner>),
            setsocketoption: None,
            sendfile: None,
            connectcontinue: None,
            reserved_fn_3: None,
            reserved_fn_2: None,
            reserved_fn_1: None,
            reserved_fn_0: None,
        };

        FileWrapper {
            methods_ref: Arc::new(methods),
            phantom: PhantomData,
        }
    }

    pub fn wrap(&self, inner: Inner) -> File
    // FIXME: is this all the bounds I need to be soundly boxed-trait-like?
    where
        Inner: Send + Sync + 'static,
    {
        let methods_raw = self.methods_ref.deref() as *const _;
        let mut boxed = Box::new(WrappedFileImpl {
            prfd: ffi::PRFileDesc {
                methods: methods_raw,
                secret: ptr::null_mut(),
                lower: ptr::null_mut(),
                higher: ptr::null_mut(),
                dtor: None,
                identity: *WRAPPED_FILE_IDENT,
            },
            _methods_ref: self.methods_ref.clone(),
            inner: inner,
        });
        unsafe {
            let raw = &mut boxed.prfd as RawFile;
            (*raw).secret = Box::into_raw(boxed) as *mut ffi::PRFilePrivate;
            File::from_raw_prfd(raw)
        }
    }
}

mod wrapper_methods {
    use super::{BorrowedFile, FileMethods, WrappedFileImpl, WRAPPED_FILE_IDENT};
    use libc::c_void;
    use nspr::bool_to_nspr;
    use nspr::error::PR_ADDRESS_NOT_SUPPORTED_ERROR;
    use nspr::net::{read_net_addr, write_net_addr};
    use nspr::time::duration_opt_from_nspr;
    use nss_sys::nspr::{
        PRBool, PRFileDesc, PRInt32, PRIntervalTime, PRIntn, PRNetAddr, PRSocketOptionCase,
        PRSocketOptionData, PRStatus, PR_SockOpt_Nonblocking, PR_FAILURE, PR_MSG_PEEK, PR_SUCCESS,
    };
    use std::mem;
    use std::slice;
    use wrap_callback;

    unsafe fn xlate_fd<Inner: FileMethods>(
        fd: *mut PRFileDesc,
    ) -> BorrowedFile<WrappedFileImpl<Inner>> {
        BorrowedFile::from_raw_prfd_checked(fd, *WRAPPED_FILE_IDENT)
    }

    pub unsafe extern "C" fn close<Inner: FileMethods>(fd: *mut PRFileDesc) -> PRStatus {
        wrap_callback(PR_FAILURE, || {
            let raw_box = {
                let mut this = xlate_fd::<Inner>(fd);
                // Ensure that, whatever in-place linked list node swapping
                // happened during this object's lifetime due to I/O layering,
                // its contents are now back where they started and we can
                // safely free the box.  (This condition will generally *not*
                // be true in other methods.)
                assert_eq!(&mut this.get_mut().prfd as *mut PRFileDesc, fd);
                this.get_mut() as *mut WrappedFileImpl<Inner>
            };
            mem::drop(Box::from_raw(raw_box));
            Ok(PR_SUCCESS)
        })
    }

    pub unsafe extern "C" fn read<Inner: FileMethods>(
        fd: *mut PRFileDesc,
        buf: *mut c_void,
        amount: PRInt32,
    ) -> PRInt32 {
        wrap_callback(-1, || {
            let this = xlate_fd::<Inner>(fd);
            assert!(amount >= 0);
            this.get_ref()
                .read(slice::from_raw_parts_mut(buf as *mut u8, amount as usize))
                .map(|len| {
                    assert!(len <= amount as usize);
                    len as PRInt32
                })
        })
    }

    pub unsafe extern "C" fn write<Inner: FileMethods>(
        fd: *mut PRFileDesc,
        buf: *const c_void,
        amount: PRInt32,
    ) -> PRInt32 {
        wrap_callback(-1, || {
            let this = xlate_fd::<Inner>(fd);
            assert!(amount >= 0);
            this.get_ref()
                .write(slice::from_raw_parts(buf as *mut u8, amount as usize))
                .map(|len| {
                    assert!(len <= amount as usize);
                    len as PRInt32
                })
        })
    }

    pub unsafe extern "C" fn connect<Inner: FileMethods>(
        fd: *mut PRFileDesc,
        addr: *const PRNetAddr,
        timeout: PRIntervalTime,
    ) -> PRStatus {
        wrap_callback(PR_FAILURE, || {
            let this = xlate_fd::<Inner>(fd);
            if let Some(rust_addr) = read_net_addr(addr) {
                this.get_ref()
                    .connect(rust_addr, duration_opt_from_nspr(timeout))
                    .map(|()| PR_SUCCESS)
            } else {
                Err(PR_ADDRESS_NOT_SUPPORTED_ERROR.into())
            }
        })
    }

    pub unsafe extern "C" fn recv<Inner: FileMethods>(
        fd: *mut PRFileDesc,
        buf: *mut c_void,
        amount: PRInt32,
        flags: PRIntn,
        timeout: PRIntervalTime,
    ) -> PRInt32 {
        wrap_callback(-1, || {
            let this = xlate_fd::<Inner>(fd);
            assert!(amount >= 0);
            let peek = flags & PR_MSG_PEEK != 0;
            this.get_ref()
                .recv(
                    slice::from_raw_parts_mut(buf as *mut u8, amount as usize),
                    peek,
                    duration_opt_from_nspr(timeout),
                )
                .map(|len| {
                    assert!(len <= amount as usize);
                    len as PRInt32
                })
        })
    }

    pub unsafe extern "C" fn send<Inner: FileMethods>(
        fd: *mut PRFileDesc,
        buf: *const c_void,
        amount: PRInt32,
        _flags: PRIntn,
        timeout: PRIntervalTime,
    ) -> PRInt32 {
        wrap_callback(-1, || {
            let this = xlate_fd::<Inner>(fd);
            assert!(amount >= 0);
            this.get_ref()
                .send(
                    slice::from_raw_parts(buf as *mut u8, amount as usize),
                    duration_opt_from_nspr(timeout),
                )
                .map(|len| {
                    assert!(len <= amount as usize);
                    len as PRInt32
                })
        })
    }

    pub unsafe extern "C" fn getsockname<Inner: FileMethods>(
        fd: *mut PRFileDesc,
        addr: *mut PRNetAddr,
    ) -> PRStatus {
        wrap_callback(PR_FAILURE, || {
            let this = xlate_fd::<Inner>(fd);
            this.get_ref().getsockname().map(|rust_addr| {
                write_net_addr(addr, rust_addr);
                PR_SUCCESS
            })
        })
    }

    pub unsafe extern "C" fn getpeername<Inner: FileMethods>(
        fd: *mut PRFileDesc,
        addr: *mut PRNetAddr,
    ) -> PRStatus {
        wrap_callback(PR_FAILURE, || {
            let this = xlate_fd::<Inner>(fd);
            this.get_ref().getpeername().map(|rust_addr| {
                write_net_addr(addr, rust_addr);
                PR_SUCCESS
            })
        })
    }

    pub unsafe extern "C" fn getsocketoption<Inner: FileMethods>(
        fd: *mut PRFileDesc,
        data: *mut PRSocketOptionData,
    ) -> PRStatus {
        wrap_callback(PR_FAILURE, || {
            let this = xlate_fd::<Inner>(fd);
            match (*data).get_enum() {
                PR_SockOpt_Nonblocking => {
                    let data = data as *mut PRSocketOptionCase<PRBool>;
                    this.get_ref().get_nonblocking().map(|b| {
                        (*data).value = bool_to_nspr(b);
                        PR_SUCCESS
                    })
                }
                _ => unimplemented!(),
            }
        })
    }
}

lazy_static! {
    static ref WRAPPED_FILE_IDENT: ffi::PRDescIdentity = {
        super::init();
        let name = CString::new("Rust").unwrap();
        unsafe { ffi::PR_GetUniqueIdentity(name.as_ptr()) }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem;

    fn pipe_test<F: FileMethods>(reader: F, writer: F) {
        static TEST: &'static str = "Testing…";

        assert_eq!(writer.write(TEST.as_bytes()).unwrap(), TEST.len());
        let mut read_buf = vec![0u8; TEST.len()];
        assert_eq!(reader.read(&mut read_buf[..4]).unwrap(), 4);
        assert_eq!(&read_buf[..4], "Test".as_bytes());
        assert_eq!(reader.read(&mut read_buf[4..]).unwrap(), TEST.len() - 4);
        assert_eq!(read_buf, TEST.as_bytes());
        mem::drop(writer);
        assert_eq!(reader.read(&mut read_buf).unwrap(), 0);
    }

    #[test]
    fn pipe_rdwr() {
        let (reader, writer) = new_pipe().unwrap();
        pipe_test(reader, writer);
    }

    #[test]
    fn wrapped_pipe_rdwr() {
        let wrapper = FileWrapper::new(PR_DESC_PIPE);
        let (reader, writer) = new_pipe().unwrap();
        pipe_test(wrapper.wrap(reader), wrapper.wrap(writer));
    }

    #[test]
    fn very_wrapped_pipe_rdwr() {
        let wrapper = FileWrapper::new(PR_DESC_PIPE);
        let (mut reader, mut writer) = new_pipe().unwrap();
        for _ in 0..100 {
            reader = wrapper.wrap(reader);
            writer = wrapper.wrap(writer);
        }
        pipe_test(reader, writer);
    }
}
