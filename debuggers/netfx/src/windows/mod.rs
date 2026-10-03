//! The ICorDebug engine (Windows only).
//!
//! Threading model:
//! - The engine thread (`dap-engine`, see [`crate::session`]) enters the multithreaded apartment and owns every
//!   ICorDebug object. It creates `ICorDebug`, attaches, sets breakpoints, walks stacks and reads values.
//! - mscordbi calls [`Callbacks`] on its own event thread. A callback only wraps its arguments, posts them to the
//!   engine's channel and returns `S_OK` without calling `Continue`. The debuggee stays stopped (ICorDebug's
//!   stop-the-world: every managed thread is held while a callback is outstanding) until the engine handles the event
//!   and calls `ICorDebugController::Continue`, or decides to stay stopped (a breakpoint). mscordbi delivers the next
//!   callback only after that `Continue`, so callbacks reach the engine one at a time.
//! - When a DAP request needs the debuggee synchronized while it runs (binding a breakpoint, detaching), the engine
//!   calls `Stop`, does the work and `Continue`s; `Stop` and `Continue` nest, so this composes with an outstanding
//!   callback.

#![allow(unsafe_code)] // COM calls into mscoree, mscordbi and the metadata importer.

mod cordebug;

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::Instant;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::ClrHosting::{
    CLRCreateInstance, CLSID_CLRDebuggingLegacy, CLSID_CLRMetaHost, ICLRMetaHost, ICLRRuntimeInfo,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
};
use windows::Win32::System::WinRT::Metadata::IMetaDataImport;
use windows_core::{BOOL, HRESULT, IUnknown, Interface, implement};

use crate::pdb::PortablePdb;
use crate::protocol::{BreakpointResult, ScopeInfo, StackFrameInfo, ThreadInfo, VariableInfo};
use crate::session::{DebugEvent, Debugger, Msg, log};
use cordebug::*;

/// Moves a COM pointer to the engine thread. ICorDebug's right-side objects are free-threaded (mscordbi requires
/// an MTA caller and locks internally), so handing one between threads is sound.
pub struct Agile<T>(T);
// SAFETY: see the type's documentation; only ICorDebug interfaces are wrapped.
unsafe impl<T> Send for Agile<T> {}

/// One marshalled callback.
pub struct CallbackEvent {
    at: Instant,
    kind: Kind,
}

enum Kind {
    CreateProcess(Agile<ICorDebugProcess>),
    ExitProcess,
    CreateAppDomain(Agile<ICorDebugAppDomain>),
    CreateThread(Agile<ICorDebugThread>),
    ExitThread(Agile<ICorDebugThread>),
    LoadModule(Agile<ICorDebugModule>),
    Breakpoint {
        thread: Agile<ICorDebugThread>,
        breakpoint: Agile<ICorDebugBreakpoint>,
    },
    Break(Agile<ICorDebugThread>),
    DebuggerError(HRESULT),
    /// Any other callback: logged and continued.
    Other(&'static str),
}

#[implement(ICorDebugManagedCallback, ICorDebugManagedCallback2)]
struct Callbacks {
    tx: Sender<Msg<CallbackEvent>>,
}

impl Callbacks {
    fn post(&self, kind: Kind) -> HRESULT {
        let _ = self.tx.send(Msg::Debugger(CallbackEvent {
            at: Instant::now(),
            kind,
        }));
        HRESULT(0)
    }
}

/// Takes a reference to a borrowed callback argument.
fn borrow<T: Interface + Clone>(raw: *mut c_void) -> Option<Agile<T>> {
    // SAFETY: mscordbi passes a valid interface pointer of type `T` (or null) for the duration of the call; cloning
    // AddRefs it so it outlives the call.
    unsafe { T::from_raw_borrowed(&raw).cloned().map(Agile) }
}

impl Callbacks {
    fn post_or_other<T: Interface + Clone>(
        &self,
        raw: *mut c_void,
        make: impl FnOnce(Agile<T>) -> Kind,
        name: &'static str,
    ) -> HRESULT {
        match borrow::<T>(raw) {
            Some(x) => self.post(make(x)),
            None => self.post(Kind::Other(name)),
        }
    }
}

impl ICorDebugManagedCallback_Impl for Callbacks_Impl {
    unsafe fn Breakpoint(&self, _ad: *mut c_void, thread: *mut c_void, bp: *mut c_void) -> HRESULT {
        match (
            borrow::<ICorDebugThread>(thread),
            borrow::<ICorDebugBreakpoint>(bp),
        ) {
            (Some(thread), Some(breakpoint)) => self.post(Kind::Breakpoint { thread, breakpoint }),
            _ => self.post(Kind::Other("Breakpoint (null argument)")),
        }
    }
    unsafe fn StepComplete(
        &self,
        _: *mut c_void,
        _: *mut c_void,
        _: *mut c_void,
        _: u32,
    ) -> HRESULT {
        self.post(Kind::Other("StepComplete"))
    }
    unsafe fn Break(&self, _ad: *mut c_void, thread: *mut c_void) -> HRESULT {
        self.post_or_other(thread, Kind::Break, "Break")
    }
    unsafe fn Exception(&self, _: *mut c_void, _: *mut c_void, _: BOOL) -> HRESULT {
        self.post(Kind::Other("Exception"))
    }
    unsafe fn EvalComplete(&self, _: *mut c_void, _: *mut c_void, _: *mut c_void) -> HRESULT {
        self.post(Kind::Other("EvalComplete"))
    }
    unsafe fn EvalException(&self, _: *mut c_void, _: *mut c_void, _: *mut c_void) -> HRESULT {
        self.post(Kind::Other("EvalException"))
    }
    unsafe fn CreateProcess(&self, process: *mut c_void) -> HRESULT {
        self.post_or_other(process, Kind::CreateProcess, "CreateProcess")
    }
    unsafe fn ExitProcess(&self, _: *mut c_void) -> HRESULT {
        self.post(Kind::ExitProcess)
    }
    unsafe fn CreateThread(&self, _ad: *mut c_void, thread: *mut c_void) -> HRESULT {
        self.post_or_other(thread, Kind::CreateThread, "CreateThread")
    }
    unsafe fn ExitThread(&self, _ad: *mut c_void, thread: *mut c_void) -> HRESULT {
        self.post_or_other(thread, Kind::ExitThread, "ExitThread")
    }
    unsafe fn LoadModule(&self, _ad: *mut c_void, module: *mut c_void) -> HRESULT {
        self.post_or_other(module, Kind::LoadModule, "LoadModule")
    }
    unsafe fn UnloadModule(&self, _: *mut c_void, _: *mut c_void) -> HRESULT {
        self.post(Kind::Other("UnloadModule"))
    }
    unsafe fn LoadClass(&self, _: *mut c_void, _: *mut c_void) -> HRESULT {
        self.post(Kind::Other("LoadClass"))
    }
    unsafe fn UnloadClass(&self, _: *mut c_void, _: *mut c_void) -> HRESULT {
        self.post(Kind::Other("UnloadClass"))
    }
    unsafe fn DebuggerError(&self, _: *mut c_void, error_hr: HRESULT, _: u32) -> HRESULT {
        self.post(Kind::DebuggerError(error_hr))
    }
    unsafe fn LogMessage(
        &self,
        _: *mut c_void,
        _: *mut c_void,
        _: i32,
        _: *mut u16,
        _: *mut u16,
    ) -> HRESULT {
        self.post(Kind::Other("LogMessage"))
    }
    unsafe fn LogSwitch(
        &self,
        _: *mut c_void,
        _: *mut c_void,
        _: i32,
        _: u32,
        _: *mut u16,
        _: *mut u16,
    ) -> HRESULT {
        self.post(Kind::Other("LogSwitch"))
    }
    unsafe fn CreateAppDomain(&self, _p: *mut c_void, ad: *mut c_void) -> HRESULT {
        self.post_or_other(ad, Kind::CreateAppDomain, "CreateAppDomain")
    }
    unsafe fn ExitAppDomain(&self, _: *mut c_void, _: *mut c_void) -> HRESULT {
        self.post(Kind::Other("ExitAppDomain"))
    }
    unsafe fn LoadAssembly(&self, _: *mut c_void, _: *mut c_void) -> HRESULT {
        self.post(Kind::Other("LoadAssembly"))
    }
    unsafe fn UnloadAssembly(&self, _: *mut c_void, _: *mut c_void) -> HRESULT {
        self.post(Kind::Other("UnloadAssembly"))
    }
    unsafe fn ControlCTrap(&self, _: *mut c_void) -> HRESULT {
        self.post(Kind::Other("ControlCTrap"))
    }
    unsafe fn NameChange(&self, _: *mut c_void, _: *mut c_void) -> HRESULT {
        self.post(Kind::Other("NameChange"))
    }
    unsafe fn UpdateModuleSymbols(
        &self,
        _: *mut c_void,
        _: *mut c_void,
        _: *mut c_void,
    ) -> HRESULT {
        self.post(Kind::Other("UpdateModuleSymbols"))
    }
    unsafe fn EditAndContinueRemap(
        &self,
        _: *mut c_void,
        _: *mut c_void,
        _: *mut c_void,
        _: BOOL,
    ) -> HRESULT {
        self.post(Kind::Other("EditAndContinueRemap"))
    }
    unsafe fn BreakpointSetError(
        &self,
        _: *mut c_void,
        _: *mut c_void,
        _: *mut c_void,
        _: u32,
    ) -> HRESULT {
        self.post(Kind::Other("BreakpointSetError"))
    }
}

impl ICorDebugManagedCallback2_Impl for Callbacks_Impl {
    unsafe fn FunctionRemapOpportunity(
        &self,
        _: *mut c_void,
        _: *mut c_void,
        _: *mut c_void,
        _: *mut c_void,
        _: u32,
    ) -> HRESULT {
        self.post(Kind::Other("FunctionRemapOpportunity"))
    }
    unsafe fn CreateConnection(&self, _: *mut c_void, _: u32, _: *mut u16) -> HRESULT {
        self.post(Kind::Other("CreateConnection"))
    }
    unsafe fn ChangeConnection(&self, _: *mut c_void, _: u32) -> HRESULT {
        self.post(Kind::Other("ChangeConnection"))
    }
    unsafe fn DestroyConnection(&self, _: *mut c_void, _: u32) -> HRESULT {
        self.post(Kind::Other("DestroyConnection"))
    }
    unsafe fn Exception(
        &self,
        _: *mut c_void,
        _: *mut c_void,
        _: *mut c_void,
        _: u32,
        _: u32,
        _: u32,
    ) -> HRESULT {
        self.post(Kind::Other("Exception2"))
    }
    unsafe fn ExceptionUnwind(&self, _: *mut c_void, _: *mut c_void, _: u32, _: u32) -> HRESULT {
        self.post(Kind::Other("ExceptionUnwind"))
    }
    unsafe fn FunctionRemapComplete(
        &self,
        _: *mut c_void,
        _: *mut c_void,
        _: *mut c_void,
    ) -> HRESULT {
        self.post(Kind::Other("FunctionRemapComplete"))
    }
    unsafe fn MDANotification(&self, _: *mut c_void, _: *mut c_void, _: *mut c_void) -> HRESULT {
        self.post(Kind::Other("MDANotification"))
    }
}

fn check(hr: HRESULT, what: &str) -> Result<(), String> {
    if hr.is_ok() {
        Ok(())
    } else {
        Err(format!(
            "{what} failed: 0x{:08x} ({})",
            hr.0 as u32,
            hr.message()
        ))
    }
}

/// Calls a method with an interface out-parameter.
fn out<T>(what: &str, f: impl FnOnce(*mut Option<T>) -> HRESULT) -> Result<T, String> {
    let mut v: Option<T> = None;
    check(f(&mut v), what)?;
    v.ok_or_else(|| format!("{what} returned null"))
}

fn same_object<A: Interface, B: Interface>(a: &A, b: &B) -> bool {
    match (a.cast::<IUnknown>(), b.cast::<IUnknown>()) {
        (Ok(a), Ok(b)) => a.as_raw() == b.as_raw(),
        _ => false,
    }
}

struct LoadedModule {
    module: ICorDebugModule,
    base: u64,
    path: String,
    pdb: Option<PortablePdb>,
    metadata: Option<IMetaDataImport>,
}

impl LoadedModule {
    fn metadata(&mut self) -> Option<&IMetaDataImport> {
        if self.metadata.is_none() {
            let mut raw: *mut c_void = std::ptr::null_mut();
            // SAFETY: GetMetaDataInterface writes an AddRef'd IMetaDataImport on success.
            let hr = unsafe {
                self.module
                    .GetMetaDataInterface(&IMetaDataImport::IID, &mut raw)
            };
            if hr.is_ok() && !raw.is_null() {
                // SAFETY: `raw` is an owned IMetaDataImport pointer.
                self.metadata = Some(unsafe { IMetaDataImport::from_raw(raw) });
            }
        }
        self.metadata.as_ref()
    }

    /// `Namespace.Type.Method` for a MethodDef token.
    fn method_name(&mut self, token: u32) -> String {
        let Some(md) = self.metadata() else {
            return format!("0x{token:08x}");
        };
        let mut name = [0u16; 512];
        let mut len = 0u32;
        let mut class = 0u32;
        // SAFETY: the buffers outlive the calls; unused out-parameters are null.
        unsafe {
            if md
                .GetMethodProps(
                    token,
                    &mut class,
                    Some(&mut name),
                    &mut len,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
                .is_err()
            {
                return format!("0x{token:08x}");
            }
            let method = wide(&name, len);
            let mut tname = [0u16; 512];
            let mut tlen = 0u32;
            if md
                .GetTypeDefProps(
                    class,
                    Some(&mut tname),
                    &mut tlen,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
                .is_ok()
            {
                format!("{}.{method}", wide(&tname, tlen))
            } else {
                method
            }
        }
    }
}

fn wide(buf: &[u16], len_with_nul: u32) -> String {
    let n = (len_with_nul as usize).min(buf.len());
    let s = &buf[..n];
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..end])
}

struct Breakpoint {
    id: i64,
    source: String,
    line: u32,
    bound: Option<(ICorDebugFunctionBreakpoint, u32)>,
}

impl Breakpoint {
    fn result(&self) -> BreakpointResult {
        BreakpointResult {
            id: self.id,
            verified: self.bound.is_some(),
            line: self.bound.as_ref().map(|b| b.1).unwrap_or(self.line),
            source_path: self.source.clone(),
            message: if self.bound.is_some() {
                None
            } else {
                Some("No loaded module has symbols for this line yet".into())
            },
        }
    }
}

struct Stop {
    /// Frames handed out by `stackTrace` during this stop; a frame id is its index + 1.
    frames: Vec<ICorDebugFrame>,
}

/// The ICorDebug-backed [`Debugger`].
pub struct CorDebugger {
    tx: Sender<Msg<CallbackEvent>>,
    cordebug: Option<ICorDebug>,
    process: Option<ICorDebugProcess>,
    process_handle: Option<HANDLE>,
    modules: Vec<LoadedModule>,
    threads: BTreeMap<u32, ICorDebugThread>,
    breakpoints: Vec<Breakpoint>,
    next_breakpoint_id: i64,
    stop: Option<Stop>,
}

impl CorDebugger {
    pub fn new(tx: Sender<Msg<CallbackEvent>>) -> Self {
        // SAFETY: plain COM initialization of this (the engine) thread; ICorDebug requires an MTA caller.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() {
            log(&format!("CoInitializeEx(MTA) failed: {hr:?}"));
        }
        Self {
            tx,
            cordebug: None,
            process: None,
            process_handle: None,
            modules: Vec::new(),
            threads: BTreeMap::new(),
            breakpoints: Vec::new(),
            next_breakpoint_id: 1,
            stop: None,
        }
    }

    fn process(&self) -> Result<&ICorDebugProcess, String> {
        self.process
            .as_ref()
            .ok_or_else(|| "not attached".to_string())
    }

    fn continue_process(&self, why: &str) {
        if let Some(p) = &self.process {
            // SAFETY: a plain COM call on an object this thread owns.
            if let Err(e) = check(unsafe { p.Continue(BOOL(0)) }, "Continue") {
                log(&format!("{e} (after {why})"));
            }
        }
    }

    /// Runs `f` with the debuggee synchronized: stops it first when it is running, continues afterwards.
    fn synchronized<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let must_stop = self.stop.is_none() && self.process.is_some();
        if must_stop {
            // SAFETY: a plain COM call on an object this thread owns.
            if let Err(e) = check(unsafe { self.process.as_ref().unwrap().Stop(0) }, "Stop") {
                log(&e);
            }
        }
        let r = f(self);
        if must_stop {
            self.continue_process("a synchronized request");
        }
        r
    }

    /// Binds every unbound breakpoint it can in module `index`. Returns the ids that bound.
    fn bind_pending(&mut self, index: usize) -> Vec<i64> {
        let mut bound = Vec::new();
        for i in 0..self.breakpoints.len() {
            if self.breakpoints[i].bound.is_some() {
                continue;
            }
            let (source, line) = (self.breakpoints[i].source.clone(), self.breakpoints[i].line);
            match bind(&self.modules[index], &source, line) {
                Ok(Some(b)) => {
                    self.breakpoints[i].bound = Some(b);
                    bound.push(self.breakpoints[i].id);
                }
                Ok(None) => {}
                Err(e) => log(&format!(
                    "breakpoint {source}:{line} in {}: {e}",
                    self.modules[index].path
                )),
            }
        }
        bound
    }

    fn module_index(&self, module: &ICorDebugModule) -> Option<usize> {
        let mut base = 0u64;
        // SAFETY: a plain COM call.
        if unsafe { module.GetBaseAddress(&mut base) }.is_err() {
            return None;
        }
        self.modules.iter().position(|m| m.base == base)
    }

    fn describe_frame(&mut self, frame: &ICorDebugFrame, id: i64) -> StackFrameInfo {
        let external = StackFrameInfo {
            id,
            name: "[External Code]".into(),
            source_path: None,
            line: 0,
            column: 0,
        };
        let Ok(il) = frame.cast::<ICorDebugILFrame>() else {
            return external;
        };
        let mut token = 0u32;
        let (mut ip, mut mapping) = (0u32, 0u32);
        // SAFETY: plain COM calls with valid out-pointers.
        let module = unsafe {
            if il.GetFunctionToken(&mut token).is_err() || il.GetIP(&mut ip, &mut mapping).is_err()
            {
                return external;
            }
            out("GetFunction", |p| frame.GetFunction(p))
                .and_then(|f| out("GetModule", |p| f.GetModule(p)))
        };
        let Some(index) = module.ok().and_then(|m| self.module_index(&m)) else {
            return external;
        };
        let m = &mut self.modules[index];
        let name = m.method_name(token);
        let point = m.pdb.as_ref().and_then(|p| p.point_at(token, ip));
        match point {
            Some(sp) => StackFrameInfo {
                id,
                name,
                source_path: m
                    .pdb
                    .as_ref()
                    .and_then(|p| p.document_name(sp.document))
                    .map(str::to_string),
                line: sp.start_line,
                column: sp.start_column,
            },
            None => StackFrameInfo {
                id,
                name,
                source_path: None,
                line: 0,
                column: 0,
            },
        }
    }

    fn frame(&self, frame_id: i64) -> Result<ICorDebugFrame, String> {
        let stop = self.stop.as_ref().ok_or("the debuggee is running")?;
        usize::try_from(frame_id - 1)
            .ok()
            .and_then(|i| stop.frames.get(i))
            .cloned()
            .ok_or_else(|| format!("no frame {frame_id} in this stop"))
    }
}

/// Binds one source line in one module: (breakpoint, bound line), or `None` when the module lacks that document.
fn bind(
    m: &LoadedModule,
    source: &str,
    line: u32,
) -> Result<Option<(ICorDebugFunctionBreakpoint, u32)>, String> {
    let Some(pdb) = &m.pdb else { return Ok(None) };
    let Some(doc) = pdb.find_document(source) else {
        return Ok(None);
    };
    let Some(b) = pdb.bind_line(doc, line) else {
        return Ok(None);
    };
    // SAFETY: plain COM calls on objects owned by this thread.
    unsafe {
        let f = out("GetFunctionFromToken", |p| {
            m.module.GetFunctionFromToken(b.method_token, p)
        })?;
        let code = out("GetILCode", |p| f.GetILCode(p))?;
        let bp = out("CreateBreakpoint", |p| {
            code.CreateBreakpoint(b.il_offset, p)
        })?;
        check(bp.Activate(BOOL(1)), "Activate")?;
        log(&format!(
            "bound {source}:{line} to 0x{:08x}+IL_{:04x} (line {}) in {}",
            b.method_token, b.il_offset, b.line, m.path
        ));
        Ok(Some((bp, b.line)))
    }
}

fn module_name(module: &ICorDebugModule) -> String {
    let mut buf = vec![0u16; 1024];
    let mut len = 0u32;
    // SAFETY: the buffer outlives the call.
    if unsafe { module.GetName(buf.len() as u32, &mut len, buf.as_mut_ptr()) }.is_ok() {
        wide(&buf, len)
    } else {
        String::new()
    }
}

/// The PDB beside a module, if it is a readable portable PDB.
fn load_pdb(module_path: &str) -> Option<PortablePdb> {
    if module_path.is_empty() {
        return None;
    }
    let pdb_path = Path::new(module_path).with_extension("pdb");
    let bytes = std::fs::read(&pdb_path).ok()?;
    match PortablePdb::parse(&bytes) {
        Ok(p) => {
            log(&format!(
                "symbols: {} ({} documents)",
                pdb_path.display(),
                p.documents().len()
            ));
            Some(p)
        }
        Err(e) => {
            log(&format!("symbols: {}: {e}", pdb_path.display()));
            None
        }
    }
}

/// The v4 runtime loaded in a process, through `ICLRMetaHost::EnumerateLoadedRuntimes`.
fn loaded_v4_runtime(
    meta: &ICLRMetaHost,
    handle: HANDLE,
    pid: u32,
) -> Result<ICLRRuntimeInfo, String> {
    // SAFETY: plain COM calls; buffers outlive them.
    unsafe {
        let runtimes = meta
            .EnumerateLoadedRuntimes(handle)
            .map_err(|e| format!("EnumerateLoadedRuntimes({pid}) failed: {e}"))?;
        let mut found = Vec::new();
        loop {
            let mut item: [Option<IUnknown>; 1] = [None];
            let mut fetched = 0u32;
            if runtimes.Next(&mut item, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(rt) = item[0]
                .take()
                .and_then(|u| u.cast::<ICLRRuntimeInfo>().ok())
            else {
                continue;
            };
            let mut buf = [0u16; 64];
            let mut len = buf.len() as u32;
            let version = if rt
                .GetVersionString(Some(windows_core::PWSTR(buf.as_mut_ptr())), &mut len)
                .is_ok()
            {
                wide(&buf, len)
            } else {
                "?".into()
            };
            if version.starts_with("v4.") {
                log(&format!("process {pid} runs CLR {version}"));
                return Ok(rt);
            }
            found.push(version);
        }
        Err(if found.is_empty() {
            format!(
                "process {pid} has no .NET Framework runtime loaded (a .NET Core or native process?)"
            )
        } else {
            format!(
                "process {pid} has CLR {} loaded; only .NET Framework 4 is supported",
                found.join(", ")
            )
        })
    }
}

impl Debugger for CorDebugger {
    type Event = CallbackEvent;

    fn attach(&mut self, pid: u32) -> Result<(), String> {
        if self.cordebug.is_some() {
            return Err("already attached".into());
        }
        let t0 = Instant::now();
        // SAFETY: COM and Win32 calls with valid arguments; every returned object is owned.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid)
                .map_err(|e| format!("cannot open process {pid}: {e}"))?;
            self.process_handle = Some(handle);
            let meta: ICLRMetaHost = CLRCreateInstance(&CLSID_CLRMetaHost)
                .map_err(|e| format!("CLRCreateInstance(CLRMetaHost) failed: {e}"))?;
            let runtime = loaded_v4_runtime(&meta, handle, pid)?;
            let cordebug: ICorDebug = runtime
                .GetInterface(&CLSID_CLRDebuggingLegacy)
                .map_err(|e| format!("ICLRRuntimeInfo::GetInterface(ICorDebug) failed: {e}"))?;
            let t_create = t0.elapsed();
            check(cordebug.Initialize(), "ICorDebug::Initialize")?;
            let callbacks: ICorDebugManagedCallback = Callbacks {
                tx: self.tx.clone(),
            }
            .into();
            check(
                cordebug.SetManagedHandler(callbacks.as_raw()),
                "ICorDebug::SetManagedHandler",
            )?;
            self.cordebug = Some(cordebug);
            let t_attach = Instant::now();
            let process = out("ICorDebug::DebugActiveProcess", |p| {
                self.cordebug
                    .as_ref()
                    .unwrap()
                    .DebugActiveProcess(pid, BOOL(0), p)
            })?;
            log(&format!(
                "attach: ICorDebug created in {:.1} ms, DebugActiveProcess {:.1} ms, total {:.1} ms",
                t_create.as_secs_f64() * 1000.0,
                t_attach.elapsed().as_secs_f64() * 1000.0,
                t0.elapsed().as_secs_f64() * 1000.0
            ));
            if self.process.is_none() {
                self.process = Some(process);
            }
        }
        Ok(())
    }

    fn set_breakpoints(
        &mut self,
        source_path: &str,
        lines: &[u32],
    ) -> Result<Vec<BreakpointResult>, String> {
        let key = source_path.replace('\\', "/").to_ascii_lowercase();
        self.synchronized(|s| {
            // Replace this source's breakpoints.
            s.breakpoints.retain(|b| {
                if b.source.replace('\\', "/").to_ascii_lowercase() != key {
                    return true;
                }
                if let Some((bp, _)) = &b.bound {
                    // SAFETY: a plain COM call.
                    let _ = unsafe { bp.Activate(BOOL(0)) };
                }
                false
            });
            let mut results = Vec::new();
            for &line in lines {
                let id = s.next_breakpoint_id;
                s.next_breakpoint_id += 1;
                s.breakpoints.push(Breakpoint {
                    id,
                    source: source_path.to_string(),
                    line,
                    bound: None,
                });
                let i = s.breakpoints.len() - 1;
                for mi in 0..s.modules.len() {
                    match bind(&s.modules[mi], source_path, line) {
                        Ok(Some(b)) => {
                            s.breakpoints[i].bound = Some(b);
                            break;
                        }
                        Ok(None) => {}
                        Err(e) => log(&format!("breakpoint {source_path}:{line}: {e}")),
                    }
                }
                results.push(s.breakpoints[i].result());
            }
            Ok(results)
        })
    }

    fn configuration_done(&mut self) -> Result<(), String> {
        // An attached debuggee was never paused; nothing to resume.
        Ok(())
    }

    fn threads(&mut self) -> Result<Vec<ThreadInfo>, String> {
        Ok(self
            .threads
            .keys()
            .map(|&id| ThreadInfo {
                id,
                name: format!("Thread {id}"),
            })
            .collect())
    }

    fn stack_trace(&mut self, thread_id: u32) -> Result<Vec<StackFrameInfo>, String> {
        if self.stop.is_none() {
            return Err("the debuggee is running".into());
        }
        let thread = match self.threads.get(&thread_id) {
            Some(t) => t.clone(),
            // SAFETY: a plain COM call.
            None => out("GetThread", |p| unsafe {
                self.process().unwrap().GetThread(thread_id, p)
            })?,
        };
        let mut raw_frames = Vec::new();
        // SAFETY: plain COM calls; enumerators hand out owned references.
        unsafe {
            let chains = out("EnumerateChains", |p| thread.EnumerateChains(p))?;
            loop {
                let mut chain: Option<ICorDebugChain> = None;
                let mut fetched = 0u32;
                if chains.Next(1, &mut chain, &mut fetched).is_err() || fetched == 0 {
                    break;
                }
                let Some(chain) = chain else { break };
                let mut managed = BOOL(0);
                let _ = chain.IsManaged(&mut managed);
                if !managed.as_bool() {
                    continue;
                }
                let frames = out("EnumerateFrames", |p| chain.EnumerateFrames(p))?;
                loop {
                    let mut frame: Option<ICorDebugFrame> = None;
                    let mut fetched = 0u32;
                    if frames.Next(1, &mut frame, &mut fetched).is_err() || fetched == 0 {
                        break;
                    }
                    if let Some(f) = frame {
                        raw_frames.push(f);
                    }
                }
            }
        }
        let mut result = Vec::with_capacity(raw_frames.len());
        for f in raw_frames {
            let stop = self.stop.as_mut().unwrap();
            stop.frames.push(f.clone());
            let id = stop.frames.len() as i64;
            result.push(self.describe_frame(&f, id));
        }
        Ok(result)
    }

    fn scopes(&mut self, frame_id: i64) -> Result<Vec<ScopeInfo>, String> {
        self.frame(frame_id)?;
        Ok(vec![ScopeInfo {
            name: "Locals".into(),
            reference: frame_id,
        }])
    }

    fn variables(&mut self, reference: i64) -> Result<Vec<VariableInfo>, String> {
        let frame = self.frame(reference)?;
        let il = frame
            .cast::<ICorDebugILFrame>()
            .map_err(|_| "not an IL frame".to_string())?;
        let mut token = 0u32;
        let (mut ip, mut mapping) = (0u32, 0u32);
        // SAFETY: plain COM calls.
        let module = unsafe {
            check(il.GetFunctionToken(&mut token), "GetFunctionToken")?;
            check(il.GetIP(&mut ip, &mut mapping), "GetIP")?;
            out("GetFunction", |p| frame.GetFunction(p))
                .and_then(|f| out("GetModule", |p| f.GetModule(p)))?
        };
        let names = self
            .module_index(&module)
            .and_then(|i| self.modules[i].pdb.as_ref())
            .map(|p| p.locals_at(token, ip))
            .unwrap_or_default();
        let mut vars = Vec::new();
        if names.is_empty() {
            // No symbols: number the slots.
            for slot in 0..64u32 {
                // SAFETY: a plain COM call.
                let Ok(v) = out("GetLocalVariable", |p| unsafe {
                    il.GetLocalVariable(slot, p)
                }) else {
                    break;
                };
                let (value, ty) = format_value(&v);
                vars.push(VariableInfo {
                    name: format!("V_{slot}"),
                    value,
                    type_name: Some(ty),
                    reference: 0,
                });
            }
        } else {
            for local in names {
                // SAFETY: a plain COM call.
                let (value, ty) = match out("GetLocalVariable", |p| unsafe {
                    il.GetLocalVariable(u32::from(local.index), p)
                }) {
                    Ok(v) => format_value(&v),
                    Err(e) => (format!("<{e}>"), "?".into()),
                };
                vars.push(VariableInfo {
                    name: local.name,
                    value,
                    type_name: Some(ty),
                    reference: 0,
                });
            }
        }
        Ok(vars)
    }

    fn continue_all(&mut self) -> Result<(), String> {
        let Some(stop) = self.stop.take() else {
            return Err("the debuggee is not stopped".into());
        };
        drop(stop);
        // SAFETY: a plain COM call.
        check(unsafe { self.process()?.Continue(BOOL(0)) }, "Continue")
    }

    fn disconnect(&mut self, terminate_debuggee: bool) -> Result<(), String> {
        let Some(process) = self.process.take() else {
            return Ok(());
        };
        let was_stopped = self.stop.take().is_some();
        // SAFETY: plain COM calls on objects this thread owns.
        let r = unsafe {
            if !was_stopped {
                check(process.Stop(0), "Stop")?;
            }
            for b in &self.breakpoints {
                if let Some((bp, _)) = &b.bound {
                    let _ = bp.Activate(BOOL(0));
                }
            }
            self.breakpoints.clear();
            if terminate_debuggee {
                check(process.Terminate(1), "Terminate")
            } else {
                check(process.Detach(), "Detach")
            }
        };
        self.threads.clear();
        self.modules.clear();
        if let Some(cd) = self.cordebug.take() {
            // SAFETY: a plain COM call; the process is detached (or terminating).
            if let Err(e) = check(unsafe { cd.Terminate() }, "ICorDebug::Terminate") {
                log(&e);
            }
        }
        if r.is_ok() {
            log(if terminate_debuggee {
                "terminated the debuggee"
            } else {
                "detached; the debuggee keeps running"
            });
        }
        r
    }

    fn on_event(&mut self, event: CallbackEvent) -> Vec<DebugEvent> {
        let CallbackEvent { at, kind } = event;
        if self.process.is_none() && !matches!(kind, Kind::CreateProcess(_)) {
            // Detached: stale events are dropped.
            return Vec::new();
        }
        match kind {
            Kind::CreateProcess(Agile(p)) => {
                if self.process.is_none() {
                    self.process = Some(p);
                }
                self.continue_process("CreateProcess");
                Vec::new()
            }
            Kind::CreateAppDomain(Agile(ad)) => {
                // SAFETY: a plain COM call.
                let _ = unsafe { ad.Attach() };
                self.continue_process("CreateAppDomain");
                Vec::new()
            }
            Kind::CreateThread(Agile(t)) => {
                let mut id = 0u32;
                // SAFETY: a plain COM call.
                if unsafe { t.GetID(&mut id) }.is_ok() {
                    self.threads.insert(id, t);
                }
                self.continue_process("CreateThread");
                Vec::new()
            }
            Kind::ExitThread(Agile(t)) => {
                let mut id = 0u32;
                // SAFETY: a plain COM call.
                if unsafe { t.GetID(&mut id) }.is_ok() {
                    self.threads.remove(&id);
                }
                self.continue_process("ExitThread");
                Vec::new()
            }
            Kind::LoadModule(Agile(m)) => {
                let path = module_name(&m);
                let mut base = 0u64;
                // SAFETY: a plain COM call.
                let _ = unsafe { m.GetBaseAddress(&mut base) };
                let pdb = load_pdb(&path);
                self.modules.push(LoadedModule {
                    module: m,
                    base,
                    path,
                    pdb,
                    metadata: None,
                });
                let ids = self.bind_pending(self.modules.len() - 1);
                self.continue_process("LoadModule");
                ids.into_iter()
                    .filter_map(|id| self.breakpoints.iter().find(|b| b.id == id))
                    .map(|b| DebugEvent::BreakpointChanged(b.result()))
                    .collect()
            }
            Kind::Breakpoint {
                thread: Agile(thread),
                breakpoint: Agile(bp),
            } => {
                let hit: Vec<i64> = self
                    .breakpoints
                    .iter()
                    .filter(|b| b.bound.as_ref().is_some_and(|(f, _)| same_object(f, &bp)))
                    .map(|b| b.id)
                    .collect();
                if hit.is_empty() {
                    // A breakpoint removed since it was hit (the callback was already queued).
                    self.continue_process("a stale Breakpoint");
                    return Vec::new();
                }
                let mut thread_id = 0u32;
                // SAFETY: a plain COM call.
                let _ = unsafe { thread.GetID(&mut thread_id) };
                self.threads.entry(thread_id).or_insert(thread);
                self.stop = Some(Stop { frames: Vec::new() });
                vec![DebugEvent::Stopped {
                    reason: "breakpoint",
                    thread_id,
                    hit_breakpoint_ids: hit,
                    observed_at: Some(at),
                }]
            }
            Kind::Break(Agile(thread)) => {
                let mut thread_id = 0u32;
                // SAFETY: a plain COM call.
                let _ = unsafe { thread.GetID(&mut thread_id) };
                self.stop = Some(Stop { frames: Vec::new() });
                vec![DebugEvent::Stopped {
                    reason: "pause",
                    thread_id,
                    hit_breakpoint_ids: Vec::new(),
                    observed_at: Some(at),
                }]
            }
            Kind::ExitProcess => {
                self.process = None;
                self.stop = None;
                let mut code = 0u32;
                if let Some(h) = self.process_handle {
                    // SAFETY: a valid process handle this debugger opened.
                    let _ = unsafe { GetExitCodeProcess(h, &mut code) };
                }
                vec![
                    DebugEvent::Exited {
                        exit_code: code as i32,
                    },
                    DebugEvent::Terminated,
                ]
            }
            Kind::DebuggerError(hr) => {
                log(&format!("DebuggerError callback: 0x{:08x}", hr.0 as u32));
                self.continue_process("DebuggerError");
                vec![DebugEvent::Output {
                    text: format!(
                        "ICorDebug reported an internal error 0x{:08x}\n",
                        hr.0 as u32
                    ),
                }]
            }
            Kind::Other(name) => {
                let _ = name;
                self.continue_process(name);
                Vec::new()
            }
        }
    }
}

impl Drop for CorDebugger {
    fn drop(&mut self) {
        if let Some(h) = self.process_handle.take() {
            // SAFETY: a handle this debugger opened and still owns.
            let _ = unsafe { CloseHandle(h) };
        }
    }
}

/// Formats a value: primitives read through `ICorDebugGenericValue`, references shown as null or an object marker.
fn format_value(v: &ICorDebugValue) -> (String, String) {
    let mut et = 0u32;
    // SAFETY: plain COM calls with buffers that outlive them.
    unsafe {
        if v.GetType(&mut et).is_err() {
            return ("<unknown>".into(), "?".into());
        }
        let ty = match et {
            0x02 => "bool",
            0x03 => "char",
            0x04 => "sbyte",
            0x05 => "byte",
            0x06 => "short",
            0x07 => "ushort",
            0x08 => "int",
            0x09 => "uint",
            0x0a => "long",
            0x0b => "ulong",
            0x0c => "float",
            0x0d => "double",
            0x18 => "nint",
            0x19 => "nuint",
            0x0e => "string",
            0x11 => "struct",
            0x12 | 0x1c => "object",
            0x14 | 0x1d => "array",
            _ => "?",
        }
        .to_string();
        if let Ok(g) = v.cast::<ICorDebugGenericValue>() {
            let mut size = 0u32;
            let _ = g.GetSize(&mut size);
            let mut buf = [0u8; 16];
            if size as usize <= buf.len() && g.GetValue(buf.as_mut_ptr().cast()).is_ok() {
                let b = buf;
                let text = match et {
                    0x02 => (b[0] != 0).to_string(),
                    0x03 => char::from_u32(u32::from(u16::from_le_bytes([b[0], b[1]])))
                        .map(|c| format!("'{c}'"))
                        .unwrap_or_else(|| "'?'".into()),
                    0x04 => (b[0] as i8).to_string(),
                    0x05 => b[0].to_string(),
                    0x06 => i16::from_le_bytes([b[0], b[1]]).to_string(),
                    0x07 => u16::from_le_bytes([b[0], b[1]]).to_string(),
                    0x08 => i32::from_le_bytes(b[..4].try_into().unwrap()).to_string(),
                    0x09 => u32::from_le_bytes(b[..4].try_into().unwrap()).to_string(),
                    0x0a | 0x18 => i64::from_le_bytes(b[..8].try_into().unwrap()).to_string(),
                    0x0b | 0x19 => u64::from_le_bytes(b[..8].try_into().unwrap()).to_string(),
                    0x0c => f32::from_le_bytes(b[..4].try_into().unwrap()).to_string(),
                    0x0d => f64::from_le_bytes(b[..8].try_into().unwrap()).to_string(),
                    _ => format!("{{{ty}}}"),
                };
                return (text, ty);
            }
        }
        if let Ok(r) = v.cast::<ICorDebugReferenceValue>() {
            let mut null = BOOL(0);
            if r.IsNull(&mut null).is_ok() && null.as_bool() {
                return ("null".into(), ty);
            }
            return (format!("{{{ty}}}"), ty);
        }
        (format!("{{{ty}}}"), ty)
    }
}
