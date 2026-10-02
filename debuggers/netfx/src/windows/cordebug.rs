//! The ICorDebug COM interfaces the spike uses, declared with `windows_core::interface`.
//!
//! The `windows` crate's metadata covers mscoree (`CLRCreateInstance`, `ICLRMetaHost`, `ICLRRuntimeInfo`) and
//! `IMetaDataImport`, but not `cordebug.idl`, so these vtables are declared here. Every method of each interface is
//! listed in vtable order, as in `cordebug.idl` (dotnet/runtime `src/coreclr/inc/cordebug.idl`, MIT; the same layout
//! ships in the .NET Framework SDK). Methods the spike never calls keep their slot with raw-pointer parameters.
//! Out-parameters that return interfaces are `*mut Option<I>`, which has the ABI of `I**`.

#![allow(non_snake_case, clippy::too_many_arguments)]

use std::ffi::c_void;

use windows_core::{BOOL, GUID, HRESULT, IUnknown, IUnknown_Vtbl, interface};

pub type CordbAddress = u64;

#[interface("3d6f5f61-7538-11d3-8d5b-00104b35e7ef")]
pub unsafe trait ICorDebug: IUnknown {
    pub fn Initialize(&self) -> HRESULT;
    pub fn Terminate(&self) -> HRESULT;
    pub fn SetManagedHandler(&self, callback: *mut c_void) -> HRESULT;
    pub fn SetUnmanagedHandler(&self, callback: *mut c_void) -> HRESULT;
    pub fn CreateProcess(
        &self,
        application_name: *const u16,
        command_line: *mut u16,
        process_attributes: *mut c_void,
        thread_attributes: *mut c_void,
        inherit_handles: BOOL,
        creation_flags: u32,
        environment: *mut c_void,
        current_directory: *const u16,
        startup_info: *mut c_void,
        process_information: *mut c_void,
        debugging_flags: u32,
        process: *mut Option<ICorDebugProcess>,
    ) -> HRESULT;
    pub fn DebugActiveProcess(
        &self,
        id: u32,
        win32_attach: BOOL,
        process: *mut Option<ICorDebugProcess>,
    ) -> HRESULT;
    pub fn EnumerateProcesses(&self, processes: *mut *mut c_void) -> HRESULT;
    pub fn GetProcess(&self, process_id: u32, process: *mut Option<ICorDebugProcess>) -> HRESULT;
    pub fn CanLaunchOrAttach(&self, process_id: u32, win32_debugging_enabled: BOOL) -> HRESULT;
}

#[interface("3d6f5f62-7538-11d3-8d5b-00104b35e7ef")]
pub unsafe trait ICorDebugController: IUnknown {
    pub fn Stop(&self, timeout_ignored: u32) -> HRESULT;
    pub fn Continue(&self, is_out_of_band: BOOL) -> HRESULT;
    pub fn IsRunning(&self, running: *mut BOOL) -> HRESULT;
    pub fn HasQueuedCallbacks(&self, thread: *mut c_void, queued: *mut BOOL) -> HRESULT;
    pub fn EnumerateThreads(&self, threads: *mut Option<ICorDebugThreadEnum>) -> HRESULT;
    pub fn SetAllThreadsDebugState(&self, state: u32, except_this_thread: *mut c_void) -> HRESULT;
    pub fn Detach(&self) -> HRESULT;
    pub fn Terminate(&self, exit_code: u32) -> HRESULT;
    pub fn CanCommitChanges(
        &self,
        snapshots: u32,
        snapshot_array: *mut c_void,
        error: *mut *mut c_void,
    ) -> HRESULT;
    pub fn CommitChanges(
        &self,
        snapshots: u32,
        snapshot_array: *mut c_void,
        error: *mut *mut c_void,
    ) -> HRESULT;
}

#[interface("3d6f5f64-7538-11d3-8d5b-00104b35e7ef")]
pub unsafe trait ICorDebugProcess: ICorDebugController {
    pub fn GetID(&self, process_id: *mut u32) -> HRESULT;
    pub fn GetHandle(&self, handle: *mut *mut c_void) -> HRESULT;
    pub fn GetThread(&self, thread_id: u32, thread: *mut Option<ICorDebugThread>) -> HRESULT;
    pub fn EnumerateObjects(&self, objects: *mut *mut c_void) -> HRESULT;
    pub fn IsTransitionStub(&self, address: CordbAddress, stub: *mut BOOL) -> HRESULT;
    pub fn IsOSSuspended(&self, thread_id: u32, suspended: *mut BOOL) -> HRESULT;
    pub fn GetThreadContext(&self, thread_id: u32, size: u32, context: *mut u8) -> HRESULT;
    pub fn SetThreadContext(&self, thread_id: u32, size: u32, context: *mut u8) -> HRESULT;
    pub fn ReadMemory(
        &self,
        address: CordbAddress,
        size: u32,
        buffer: *mut u8,
        read: *mut usize,
    ) -> HRESULT;
    pub fn WriteMemory(
        &self,
        address: CordbAddress,
        size: u32,
        buffer: *const u8,
        written: *mut usize,
    ) -> HRESULT;
    pub fn ClearCurrentException(&self, thread_id: u32) -> HRESULT;
    pub fn EnableLogMessages(&self, on: BOOL) -> HRESULT;
    pub fn ModifyLogSwitch(&self, name: *mut u16, level: i32) -> HRESULT;
    pub fn EnumerateAppDomains(&self, app_domains: *mut *mut c_void) -> HRESULT;
    pub fn GetObject(&self, object: *mut *mut c_void) -> HRESULT;
    pub fn ThreadForFiberCookie(&self, cookie: u32, thread: *mut *mut c_void) -> HRESULT;
    pub fn GetHelperThreadID(&self, thread_id: *mut u32) -> HRESULT;
}

#[interface("3d6f5f63-7538-11d3-8d5b-00104b35e7ef")]
pub unsafe trait ICorDebugAppDomain: ICorDebugController {
    pub fn GetProcess(&self, process: *mut Option<ICorDebugProcess>) -> HRESULT;
    pub fn EnumerateAssemblies(&self, assemblies: *mut *mut c_void) -> HRESULT;
    pub fn GetModuleFromMetaDataInterface(
        &self,
        metadata: *mut c_void,
        module: *mut *mut c_void,
    ) -> HRESULT;
    pub fn EnumerateBreakpoints(&self, breakpoints: *mut *mut c_void) -> HRESULT;
    pub fn EnumerateSteppers(&self, steppers: *mut *mut c_void) -> HRESULT;
    pub fn IsAttached(&self, attached: *mut BOOL) -> HRESULT;
    pub fn GetName(&self, cch: u32, pcch: *mut u32, name: *mut u16) -> HRESULT;
    pub fn GetObject(&self, object: *mut *mut c_void) -> HRESULT;
    pub fn Attach(&self) -> HRESULT;
    pub fn GetID(&self, id: *mut u32) -> HRESULT;
}

#[interface("938c6d66-7fb6-4f69-b389-425b8987329b")]
pub unsafe trait ICorDebugThread: IUnknown {
    pub fn GetProcess(&self, process: *mut Option<ICorDebugProcess>) -> HRESULT;
    pub fn GetID(&self, thread_id: *mut u32) -> HRESULT;
    pub fn GetHandle(&self, handle: *mut *mut c_void) -> HRESULT;
    pub fn GetAppDomain(&self, app_domain: *mut Option<ICorDebugAppDomain>) -> HRESULT;
    pub fn SetDebugState(&self, state: u32) -> HRESULT;
    pub fn GetDebugState(&self, state: *mut u32) -> HRESULT;
    pub fn GetUserState(&self, state: *mut u32) -> HRESULT;
    pub fn GetCurrentException(&self, exception: *mut Option<ICorDebugValue>) -> HRESULT;
    pub fn ClearCurrentException(&self) -> HRESULT;
    pub fn CreateStepper(&self, stepper: *mut *mut c_void) -> HRESULT;
    pub fn EnumerateChains(&self, chains: *mut Option<ICorDebugChainEnum>) -> HRESULT;
    pub fn GetActiveChain(&self, chain: *mut Option<ICorDebugChain>) -> HRESULT;
    pub fn GetActiveFrame(&self, frame: *mut Option<ICorDebugFrame>) -> HRESULT;
    pub fn GetRegisterSet(&self, registers: *mut *mut c_void) -> HRESULT;
    pub fn CreateEval(&self, eval: *mut *mut c_void) -> HRESULT;
    pub fn GetObject(&self, object: *mut Option<ICorDebugValue>) -> HRESULT;
}

#[interface("CC7BCAEE-8A68-11d2-983C-0000F808342D")]
pub unsafe trait ICorDebugChain: IUnknown {
    pub fn GetThread(&self, thread: *mut Option<ICorDebugThread>) -> HRESULT;
    pub fn GetStackRange(&self, start: *mut CordbAddress, end: *mut CordbAddress) -> HRESULT;
    pub fn GetContext(&self, context: *mut *mut c_void) -> HRESULT;
    pub fn GetCaller(&self, chain: *mut Option<ICorDebugChain>) -> HRESULT;
    pub fn GetCallee(&self, chain: *mut Option<ICorDebugChain>) -> HRESULT;
    pub fn GetPrevious(&self, chain: *mut Option<ICorDebugChain>) -> HRESULT;
    pub fn GetNext(&self, chain: *mut Option<ICorDebugChain>) -> HRESULT;
    pub fn IsManaged(&self, managed: *mut BOOL) -> HRESULT;
    pub fn EnumerateFrames(&self, frames: *mut Option<ICorDebugFrameEnum>) -> HRESULT;
    pub fn GetActiveFrame(&self, frame: *mut Option<ICorDebugFrame>) -> HRESULT;
    pub fn GetRegisterSet(&self, registers: *mut *mut c_void) -> HRESULT;
    pub fn GetReason(&self, reason: *mut u32) -> HRESULT;
}

#[interface("CC7BCAEF-8A68-11d2-983C-0000F808342D")]
pub unsafe trait ICorDebugFrame: IUnknown {
    pub fn GetChain(&self, chain: *mut Option<ICorDebugChain>) -> HRESULT;
    pub fn GetCode(&self, code: *mut Option<ICorDebugCode>) -> HRESULT;
    pub fn GetFunction(&self, function: *mut Option<ICorDebugFunction>) -> HRESULT;
    pub fn GetFunctionToken(&self, token: *mut u32) -> HRESULT;
    pub fn GetStackRange(&self, start: *mut CordbAddress, end: *mut CordbAddress) -> HRESULT;
    pub fn GetCaller(&self, frame: *mut Option<ICorDebugFrame>) -> HRESULT;
    pub fn GetCallee(&self, frame: *mut Option<ICorDebugFrame>) -> HRESULT;
    pub fn CreateStepper(&self, stepper: *mut *mut c_void) -> HRESULT;
}

#[interface("03E26311-4F76-11d3-88C6-006097945418")]
pub unsafe trait ICorDebugILFrame: ICorDebugFrame {
    pub fn GetIP(&self, offset: *mut u32, mapping: *mut u32) -> HRESULT;
    pub fn SetIP(&self, offset: u32) -> HRESULT;
    pub fn EnumerateLocalVariables(&self, values: *mut Option<ICorDebugValueEnum>) -> HRESULT;
    pub fn GetLocalVariable(&self, index: u32, value: *mut Option<ICorDebugValue>) -> HRESULT;
    pub fn EnumerateArguments(&self, values: *mut Option<ICorDebugValueEnum>) -> HRESULT;
    pub fn GetArgument(&self, index: u32, value: *mut Option<ICorDebugValue>) -> HRESULT;
    pub fn GetStackDepth(&self, depth: *mut u32) -> HRESULT;
    pub fn GetStackValue(&self, index: u32, value: *mut Option<ICorDebugValue>) -> HRESULT;
    pub fn CanSetIP(&self, offset: u32) -> HRESULT;
}

#[interface("CC7BCAF3-8A68-11d2-983C-0000F808342D")]
pub unsafe trait ICorDebugFunction: IUnknown {
    pub fn GetModule(&self, module: *mut Option<ICorDebugModule>) -> HRESULT;
    pub fn GetClass(&self, class: *mut *mut c_void) -> HRESULT;
    pub fn GetToken(&self, token: *mut u32) -> HRESULT;
    pub fn GetILCode(&self, code: *mut Option<ICorDebugCode>) -> HRESULT;
    pub fn GetNativeCode(&self, code: *mut Option<ICorDebugCode>) -> HRESULT;
    pub fn CreateBreakpoint(&self, breakpoint: *mut Option<ICorDebugFunctionBreakpoint>)
    -> HRESULT;
    pub fn GetLocalVarSigToken(&self, token: *mut u32) -> HRESULT;
    pub fn GetCurrentVersionNumber(&self, version: *mut u32) -> HRESULT;
}

#[interface("dba2d8c1-e5c5-4069-8c13-10a7c6abf43d")]
pub unsafe trait ICorDebugModule: IUnknown {
    pub fn GetProcess(&self, process: *mut Option<ICorDebugProcess>) -> HRESULT;
    pub fn GetBaseAddress(&self, address: *mut CordbAddress) -> HRESULT;
    pub fn GetAssembly(&self, assembly: *mut *mut c_void) -> HRESULT;
    pub fn GetName(&self, cch: u32, pcch: *mut u32, name: *mut u16) -> HRESULT;
    pub fn EnableJITDebugging(&self, track_jit_info: BOOL, allow_jit_opts: BOOL) -> HRESULT;
    pub fn EnableClassLoadCallbacks(&self, on: BOOL) -> HRESULT;
    pub fn GetFunctionFromToken(
        &self,
        method_def: u32,
        function: *mut Option<ICorDebugFunction>,
    ) -> HRESULT;
    pub fn GetFunctionFromRVA(
        &self,
        rva: CordbAddress,
        function: *mut Option<ICorDebugFunction>,
    ) -> HRESULT;
    pub fn GetClassFromToken(&self, type_def: u32, class: *mut *mut c_void) -> HRESULT;
    pub fn CreateBreakpoint(&self, breakpoint: *mut *mut c_void) -> HRESULT;
    pub fn GetEditAndContinueSnapshot(&self, snapshot: *mut *mut c_void) -> HRESULT;
    pub fn GetMetaDataInterface(&self, riid: *const GUID, object: *mut *mut c_void) -> HRESULT;
    pub fn GetToken(&self, token: *mut u32) -> HRESULT;
    pub fn IsDynamic(&self, dynamic: *mut BOOL) -> HRESULT;
    pub fn GetGlobalVariableValue(
        &self,
        field_def: u32,
        value: *mut Option<ICorDebugValue>,
    ) -> HRESULT;
    pub fn GetSize(&self, bytes: *mut u32) -> HRESULT;
    pub fn IsInMemory(&self, in_memory: *mut BOOL) -> HRESULT;
}

#[interface("CC7BCAF4-8A68-11d2-983C-0000F808342D")]
pub unsafe trait ICorDebugCode: IUnknown {
    pub fn IsIL(&self, il: *mut BOOL) -> HRESULT;
    pub fn GetFunction(&self, function: *mut Option<ICorDebugFunction>) -> HRESULT;
    pub fn GetAddress(&self, start: *mut CordbAddress) -> HRESULT;
    pub fn GetSize(&self, bytes: *mut u32) -> HRESULT;
    pub fn CreateBreakpoint(
        &self,
        offset: u32,
        breakpoint: *mut Option<ICorDebugFunctionBreakpoint>,
    ) -> HRESULT;
    pub fn GetCode(
        &self,
        start: u32,
        end: u32,
        alloc: u32,
        buffer: *mut u8,
        size: *mut u32,
    ) -> HRESULT;
    pub fn GetVersionNumber(&self, version: *mut u32) -> HRESULT;
    pub fn GetILToNativeMapping(&self, cmap: u32, pcmap: *mut u32, map: *mut c_void) -> HRESULT;
    pub fn GetEnCRemapSequencePoints(
        &self,
        cmap: u32,
        pcmap: *mut u32,
        offsets: *mut u32,
    ) -> HRESULT;
}

#[interface("CC7BCAE8-8A68-11d2-983C-0000F808342D")]
pub unsafe trait ICorDebugBreakpoint: IUnknown {
    pub fn Activate(&self, active: BOOL) -> HRESULT;
    pub fn IsActive(&self, active: *mut BOOL) -> HRESULT;
}

#[interface("CC7BCAE9-8A68-11d2-983C-0000F808342D")]
pub unsafe trait ICorDebugFunctionBreakpoint: ICorDebugBreakpoint {
    pub fn GetFunction(&self, function: *mut Option<ICorDebugFunction>) -> HRESULT;
    pub fn GetOffset(&self, offset: *mut u32) -> HRESULT;
}

#[interface("CC7BCAF7-8A68-11d2-983C-0000F808342D")]
pub unsafe trait ICorDebugValue: IUnknown {
    pub fn GetType(&self, element_type: *mut u32) -> HRESULT;
    pub fn GetSize(&self, size: *mut u32) -> HRESULT;
    pub fn GetAddress(&self, address: *mut CordbAddress) -> HRESULT;
    pub fn CreateBreakpoint(&self, breakpoint: *mut *mut c_void) -> HRESULT;
}

#[interface("CC7BCAF8-8A68-11d2-983C-0000F808342D")]
pub unsafe trait ICorDebugGenericValue: ICorDebugValue {
    pub fn GetValue(&self, to: *mut c_void) -> HRESULT;
    pub fn SetValue(&self, from: *mut c_void) -> HRESULT;
}

#[interface("CC7BCAF9-8A68-11d2-983C-0000F808342D")]
pub unsafe trait ICorDebugReferenceValue: ICorDebugValue {
    pub fn IsNull(&self, null: *mut BOOL) -> HRESULT;
    pub fn GetValue(&self, value: *mut CordbAddress) -> HRESULT;
    pub fn SetValue(&self, value: CordbAddress) -> HRESULT;
    pub fn Dereference(&self, value: *mut Option<ICorDebugValue>) -> HRESULT;
    pub fn DereferenceStrong(&self, value: *mut Option<ICorDebugValue>) -> HRESULT;
}

#[interface("CC7BCB01-8A68-11d2-983C-0000F808342D")]
pub unsafe trait ICorDebugEnum: IUnknown {
    pub fn Skip(&self, celt: u32) -> HRESULT;
    pub fn Reset(&self) -> HRESULT;
    pub fn Clone(&self, copy: *mut *mut c_void) -> HRESULT;
    pub fn GetCount(&self, count: *mut u32) -> HRESULT;
}

#[interface("CC7BCB07-8A68-11d2-983C-0000F808342D")]
pub unsafe trait ICorDebugFrameEnum: ICorDebugEnum {
    pub fn Next(
        &self,
        celt: u32,
        frames: *mut Option<ICorDebugFrame>,
        fetched: *mut u32,
    ) -> HRESULT;
}

#[interface("CC7BCB0A-8A68-11d2-983C-0000F808342D")]
pub unsafe trait ICorDebugValueEnum: ICorDebugEnum {
    pub fn Next(
        &self,
        celt: u32,
        values: *mut Option<ICorDebugValue>,
        fetched: *mut u32,
    ) -> HRESULT;
}

#[interface("CC7BCB06-8A68-11d2-983C-0000F808342D")]
pub unsafe trait ICorDebugThreadEnum: ICorDebugEnum {
    pub fn Next(
        &self,
        celt: u32,
        threads: *mut Option<ICorDebugThread>,
        fetched: *mut u32,
    ) -> HRESULT;
}

#[interface("CC7BCB08-8A68-11d2-983C-0000F808342D")]
pub unsafe trait ICorDebugChainEnum: ICorDebugEnum {
    pub fn Next(
        &self,
        celt: u32,
        chains: *mut Option<ICorDebugChain>,
        fetched: *mut u32,
    ) -> HRESULT;
}

/// The callback interface mscordbi calls on its own event thread. Every parameter is the raw interface pointer,
/// borrowed for the call.
#[interface("3d6f5f60-7538-11d3-8d5b-00104b35e7ef")]
pub unsafe trait ICorDebugManagedCallback: IUnknown {
    pub fn Breakpoint(
        &self,
        app_domain: *mut c_void,
        thread: *mut c_void,
        breakpoint: *mut c_void,
    ) -> HRESULT;
    pub fn StepComplete(
        &self,
        app_domain: *mut c_void,
        thread: *mut c_void,
        stepper: *mut c_void,
        reason: u32,
    ) -> HRESULT;
    pub fn Break(&self, app_domain: *mut c_void, thread: *mut c_void) -> HRESULT;
    pub fn Exception(
        &self,
        app_domain: *mut c_void,
        thread: *mut c_void,
        unhandled: BOOL,
    ) -> HRESULT;
    pub fn EvalComplete(
        &self,
        app_domain: *mut c_void,
        thread: *mut c_void,
        eval: *mut c_void,
    ) -> HRESULT;
    pub fn EvalException(
        &self,
        app_domain: *mut c_void,
        thread: *mut c_void,
        eval: *mut c_void,
    ) -> HRESULT;
    pub fn CreateProcess(&self, process: *mut c_void) -> HRESULT;
    pub fn ExitProcess(&self, process: *mut c_void) -> HRESULT;
    pub fn CreateThread(&self, app_domain: *mut c_void, thread: *mut c_void) -> HRESULT;
    pub fn ExitThread(&self, app_domain: *mut c_void, thread: *mut c_void) -> HRESULT;
    pub fn LoadModule(&self, app_domain: *mut c_void, module: *mut c_void) -> HRESULT;
    pub fn UnloadModule(&self, app_domain: *mut c_void, module: *mut c_void) -> HRESULT;
    pub fn LoadClass(&self, app_domain: *mut c_void, class: *mut c_void) -> HRESULT;
    pub fn UnloadClass(&self, app_domain: *mut c_void, class: *mut c_void) -> HRESULT;
    pub fn DebuggerError(
        &self,
        process: *mut c_void,
        error_hr: HRESULT,
        error_code: u32,
    ) -> HRESULT;
    pub fn LogMessage(
        &self,
        app_domain: *mut c_void,
        thread: *mut c_void,
        level: i32,
        log_switch_name: *mut u16,
        message: *mut u16,
    ) -> HRESULT;
    pub fn LogSwitch(
        &self,
        app_domain: *mut c_void,
        thread: *mut c_void,
        level: i32,
        reason: u32,
        log_switch_name: *mut u16,
        parent_name: *mut u16,
    ) -> HRESULT;
    pub fn CreateAppDomain(&self, process: *mut c_void, app_domain: *mut c_void) -> HRESULT;
    pub fn ExitAppDomain(&self, process: *mut c_void, app_domain: *mut c_void) -> HRESULT;
    pub fn LoadAssembly(&self, app_domain: *mut c_void, assembly: *mut c_void) -> HRESULT;
    pub fn UnloadAssembly(&self, app_domain: *mut c_void, assembly: *mut c_void) -> HRESULT;
    pub fn ControlCTrap(&self, process: *mut c_void) -> HRESULT;
    pub fn NameChange(&self, app_domain: *mut c_void, thread: *mut c_void) -> HRESULT;
    pub fn UpdateModuleSymbols(
        &self,
        app_domain: *mut c_void,
        module: *mut c_void,
        symbol_stream: *mut c_void,
    ) -> HRESULT;
    pub fn EditAndContinueRemap(
        &self,
        app_domain: *mut c_void,
        thread: *mut c_void,
        function: *mut c_void,
        accurate: BOOL,
    ) -> HRESULT;
    pub fn BreakpointSetError(
        &self,
        app_domain: *mut c_void,
        thread: *mut c_void,
        breakpoint: *mut c_void,
        error: u32,
    ) -> HRESULT;
}

/// Required since CLR v2: `SetManagedHandler` fails with `E_NOINTERFACE` without it.
#[interface("250E5EEA-DB5C-4C76-B6F3-8C46F12E3203")]
pub unsafe trait ICorDebugManagedCallback2: IUnknown {
    pub fn FunctionRemapOpportunity(
        &self,
        app_domain: *mut c_void,
        thread: *mut c_void,
        old_function: *mut c_void,
        new_function: *mut c_void,
        old_il_offset: u32,
    ) -> HRESULT;
    pub fn CreateConnection(
        &self,
        process: *mut c_void,
        connection_id: u32,
        name: *mut u16,
    ) -> HRESULT;
    pub fn ChangeConnection(&self, process: *mut c_void, connection_id: u32) -> HRESULT;
    pub fn DestroyConnection(&self, process: *mut c_void, connection_id: u32) -> HRESULT;
    pub fn Exception(
        &self,
        app_domain: *mut c_void,
        thread: *mut c_void,
        frame: *mut c_void,
        offset: u32,
        event_type: u32,
        flags: u32,
    ) -> HRESULT;
    pub fn ExceptionUnwind(
        &self,
        app_domain: *mut c_void,
        thread: *mut c_void,
        event_type: u32,
        flags: u32,
    ) -> HRESULT;
    pub fn FunctionRemapComplete(
        &self,
        app_domain: *mut c_void,
        thread: *mut c_void,
        function: *mut c_void,
    ) -> HRESULT;
    pub fn MDANotification(
        &self,
        controller: *mut c_void,
        thread: *mut c_void,
        mda: *mut c_void,
    ) -> HRESULT;
}
