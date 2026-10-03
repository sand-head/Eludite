using System.Diagnostics;
using System.Globalization;
using System.Net;
using System.Reflection;
using Eludite.Debugger.Mono.Protocol;
using Mono.Debugging.Client;
using Mono.Debugging.Soft;
using Newtonsoft.Json.Linq;

namespace Eludite.Debugger.Mono;

/// <summary>
/// The DAP requests over one Mono.Debugging.Soft session (protocol/schemas/dap-mono.md). Every handler and every
/// callback of the session runs on the dispatcher thread: the session's events are posted to it, so the state below
/// needs no lock.
/// </summary>
internal sealed class MonoAdapter
{
    private const int DefaultEvaluationTimeoutMs = 3000;

    /// <summary>How long a detach waits for the debuggee's agent to answer before the adapter exits anyway.</summary>
    private const int DetachTimeoutMs = 1000;

    private readonly Dispatcher _d;
    private readonly Log _log;
    private readonly HandleTable<object> _variables = new();
    private readonly HandleTable<FrameRef> _frames = new();
    private readonly Dictionary<BreakEvent, int> _breakpointIds = new();
    private readonly Dictionary<string, List<Breakpoint>> _sourceBreakpoints = new(StringComparer.Ordinal);
    private readonly List<FunctionBreakpoint> _functionBreakpoints = new();
    private readonly Dictionary<FunctionBreakpoint, FunctionSpec> _functionSpecs = new();
    private readonly List<Catchpoint> _catchpoints = new();
    private readonly Dictionary<long, Backtrace> _backtraces = new();
    private readonly EvaluationOptions _evaluation;

    /// <summary>Set on the debugger library's event thread when the session reports the target gone (a detach ends with it).</summary>
    private readonly ManualResetEventSlim _targetGone = new(false);

    private SoftDebuggerSession? _session;
    private MonoEvaluator? _evaluator;
    private SoftDebuggerStartInfo? _startInfo;
    private DebuggerSessionOptions? _sessionOptions;
    private TargetProcess? _process;
    private string? _program;
    private bool _attach;
    private bool _justMyCode = true;
    private bool _started;
    private bool _stopped;
    private bool _exitedSent;
    private bool _terminatedSent;
    private bool _ended;
    private bool _detached;
    private bool _pausing;
    private bool _unhandledStop;
    private ThreadInfo? _stopThread;
    private ThreadInfo[]? _threads;
    private FunctionBreakpoint? _entryBreakpoint;
    private (long Thread, int Depth, string Method, string File, int Line)? _stepInFrom;
    private int _stepInRetries;
    private ExceptionSettings _exceptions = ExceptionSettings.Parse(new JObject { ["filters"] = new JArray(ExceptionSettings.UserUnhandled) });
    private int _nextBreakpointId = 1;
    private int _lineBase = 1;
    private int _columnBase = 1;

    public MonoAdapter(Dispatcher dispatcher, Log log)
    {
        _d = dispatcher;
        _log = log;
        _evaluation = EvaluationOptions.DefaultOptions.Clone();
        _evaluation.EllipsizeStrings = false;
        _evaluation.AllowTargetInvoke = true;
        _evaluation.AllowMethodEvaluation = true;
        _evaluation.AllowToStringCalls = true;
        _evaluation.FlattenHierarchy = true;
        _evaluation.GroupPrivateMembers = false;
        _evaluation.GroupStaticMembers = true;
        _evaluation.UseExternalTypeResolver = true;
        _evaluation.EvaluationTimeout = DefaultEvaluationTimeoutMs;
        _evaluation.MemberEvaluationTimeout = DefaultEvaluationTimeoutMs;

        _d.Register("initialize", Initialize);
        _d.Register("launch", Launch);
        _d.Register("attach", Attach);
        _d.Register("setBreakpoints", SetBreakpoints);
        _d.Register("setFunctionBreakpoints", SetFunctionBreakpoints);
        _d.Register("setExceptionBreakpoints", SetExceptionBreakpoints);
        _d.Register("configurationDone", ConfigurationDone);
        _d.Register("threads", Threads);
        _d.Register("stackTrace", StackTrace);
        _d.Register("scopes", Scopes);
        _d.Register("variables", Variables);
        _d.Register("evaluate", Evaluate);
        _d.Register("setVariable", SetVariable);
        _d.Register("continue", a => Resume(a, s => s.Continue()));
        _d.Register("next", a => Resume(a, s => s.NextLine()));
        _d.Register("stepIn", StepIn);
        _d.Register("stepOut", a => Resume(a, s => s.Finish()));
        _d.Register("pause", Pause);
        _d.Register("exceptionInfo", ExceptionInfoRequest);
        _d.Register("disconnect", Disconnect);
        _d.Register("terminate", Terminate);
        _d.Closed += () => EndSession(terminate: !_attach);
    }

    /// <summary>The client went away or the dispatcher stopped: never leave a launched debuggee behind.</summary>
    public void Shutdown() => EndSession(terminate: !_attach);

    private SoftDebuggerSession Session => _session ?? throw new DapException("send initialize first");

    // ----- Lifecycle -----

    private JToken? Initialize(JObject args)
    {
        if (_session is not null)
        {
            throw new DapException("initialize was already sent");
        }

        _lineBase = Json.Bool(args, "linesStartAt1") is false ? 0 : 1;
        _columnBase = Json.Bool(args, "columnsStartAt1") is false ? 0 : 1;
        var s = new SoftDebuggerSession();
        s.OutputWriter = (isStderr, text) => _d.Post(() => Output(isStderr ? "stderr" : "stdout", text));
        s.LogWriter = (isStderr, text) => _d.Post(() => Output("console", text));
        s.DebugWriter = (level, category, message) => _d.Post(() => Output("console", message));
        s.ExceptionHandler = ex =>
        {
            _log.Write("Mono.Debugging: " + ex);
            return true;
        };
        s.BreakpointTraceHandler = (be, trace) => _d.Post(() => Output("console", trace.EndsWith("\n", StringComparison.Ordinal) ? trace : trace + "\n"));
        EvaluateSynchronously(s);
        var evaluator = new MonoEvaluator(s);
        _evaluator = evaluator;
        s.GetExpressionEvaluator = extension => evaluator;
        s.TypeResolverHandler = evaluator.ResolveType;
        s.TargetEvent += (sender, e) =>
        {
            if (e.Type == TargetEventType.TargetExited)
            {
                _targetGone.Set();
            }

            _d.Post(() => OnTargetEvent(e));
        };
        s.Breakpoints.BreakEventStatusChanged += (sender, e) => _d.Post(() => OnBreakEventStatus(e.BreakEvent));
        _session = s;
        return Capabilities.Initialize();
    }

    /// <summary>
    /// Mono.Debugging creates every value (each local, each member, each evaluation) on an evaluator thread and waits for
    /// it with <c>WaitHandle.WaitAny</c>, which wakes 3 to 6 ms late under Mono 6.8 (measured: 0.05 ms for WaitOne, 6 ms
    /// for WaitAny per round trip), so a frame with 200 locals took 2 s. The adapter evaluates on its dispatcher thread
    /// instead: the library's own timed evaluator in synchronous mode (<c>TimedEvaluator(useTimeout: false)</c>). A call
    /// into the debuggee still ends at <see cref="EvaluationOptions.EvaluationTimeout"/>: the soft debugger aborts the
    /// invocation then. Done by reflection on the pinned 2017 build; without it the adapter is only slower.
    /// </summary>
    private void EvaluateSynchronously(SoftDebuggerSession s)
    {
        try
        {
            var adaptor = s.Adaptor;
            var trackerField = typeof(global::Mono.Debugging.Evaluation.ObjectValueAdaptor).GetField("asyncEvaluationTracker", BindingFlags.Instance | BindingFlags.NonPublic);
            var tracker = trackerField?.GetValue(adaptor);
            var runnerField = tracker?.GetType().GetField("runner", BindingFlags.Instance | BindingFlags.NonPublic);
            if (tracker is null || runnerField is null)
            {
                _log.Write("Mono.Debugging has no asyncEvaluationTracker.runner: values are created on its evaluator thread");
                return;
            }

            (runnerField.GetValue(tracker) as IDisposable)?.Dispose();
            runnerField.SetValue(tracker, new global::Mono.Debugging.Evaluation.TimedEvaluator(false));
        }
        catch (Exception e) when (e is FieldAccessException or ArgumentException or TargetException)
        {
            _log.Write("cannot make Mono.Debugging evaluate synchronously: " + e.Message);
        }
    }

    private JToken? Launch(JObject args)
    {
        var s = Session;
        if (_startInfo is not null)
        {
            throw new DapException("launch or attach was already sent");
        }

        var a = LaunchArguments.Parse(args, Environment.CurrentDirectory);
        if (!File.Exists(a.Program))
        {
            throw new DapException(a.Program + " does not exist: build the project first");
        }

        if (!Directory.Exists(a.Cwd))
        {
            throw new DapException("the working directory " + a.Cwd + " does not exist");
        }

        var mono = ResolveMono(a.RuntimeExecutable);
        var prefix = Path.GetDirectoryName(Path.GetDirectoryName(mono)) ?? "/usr";
        var launch = new SoftDebuggerLaunchArgs(prefix, new Dictionary<string, string>())
        {
            MonoExecutableFileName = mono,
            ExternalConsoleLauncher = StartTarget,
        };
        var info = new SoftDebuggerStartInfo(launch)
        {
            Command = a.Program,
            Arguments = CommandLine.Join(a.Args),
            WorkingDirectory = a.Cwd,
            RuntimeArguments = CommandLine.Join(a.RuntimeArgs),
            UseExternalConsole = true,
        };
        foreach (var kv in a.Env)
        {
            info.EnvironmentVariables[kv.Key] = kv.Value;
        }

        _program = a.Program;
        _startInfo = info;
        _sessionOptions = SessionOptions(a.JustMyCode);
        _justMyCode = a.JustMyCode;
        if (a.StopAtEntry)
        {
            _entryBreakpoint = EntryBreakpoint(a.Program);
            if (_entryBreakpoint is not null)
            {
                s.Breakpoints.Add(_entryBreakpoint);
            }
        }

        _log.Write("launch " + mono + " " + a.Program + " " + info.Arguments);
        _d.Post(() => _d.SendEvent("initialized"));
        return null;
    }

    private JToken? Attach(JObject args)
    {
        _ = Session;
        if (_startInfo is not null)
        {
            throw new DapException("launch or attach was already sent");
        }

        var a = AttachArguments.Parse(args);
        IPAddress address;
        if (!IPAddress.TryParse(a.Address, out address!))
        {
            address = Dns.GetHostAddresses(a.Address).FirstOrDefault()
                ?? throw new DapException("cannot resolve " + a.Address);
        }

        var connect = new SoftDebuggerConnectArgs(string.Empty, address, a.Port)
        {
            MaxConnectionAttempts = 20,
            TimeBetweenConnectionAttempts = 250,
        };
        _attach = true;
        _startInfo = new SoftDebuggerStartInfo(connect);
        _sessionOptions = SessionOptions(justMyCode: true);
        _log.Write("attach " + a.Address + ":" + a.Port.ToString(CultureInfo.InvariantCulture));
        _d.Post(() => _d.SendEvent("initialized"));
        return null;
    }

    private JToken? ConfigurationDone(JObject args)
    {
        var s = Session;
        if (_startInfo is null || _sessionOptions is null)
        {
            throw new DapException("send launch or attach before configurationDone");
        }

        if (_started)
        {
            return null;
        }

        _started = true;
        s.Run(_startInfo, _sessionOptions);
        return null;
    }

    private JToken? Disconnect(JObject args)
    {
        var terminate = Json.Bool(args, "terminateDebuggee") ?? !_attach;
        EndSession(terminate);
        SendTerminated();
        _d.Post(_d.Stop);
        return null;
    }

    private JToken? Terminate(JObject args)
    {
        if (_session is null || !_started || _exitedSent)
        {
            SendTerminated();
            return null;
        }

        _stopped = false;
        ClearStop();
        try
        {
            _session.Exit();
        }
        catch (Exception e) when (e is InvalidOperationException or IOException)
        {
            _log.Write("exit: " + e.Message);
        }

        if (_process is not null && !_process.WaitForExit(3000))
        {
            _process.Kill();
        }

        return null;
    }

    /// <summary>
    /// End the session once: <c>disconnect</c>, the client closing the channel and <see cref="Shutdown"/> all come here.
    /// </summary>
    /// <remarks>
    /// The detach used to leave the adapter spinning at about 90% of a core instead of exiting (brief 0027 report,
    /// section 8, item 10). It ran twice (the <c>disconnect</c> handler, then <see cref="Shutdown"/> when the dispatcher
    /// stopped), and Mono.Debugging's <see cref="DebuggerSession.Detach"/> only queues the work on the thread pool, so
    /// the adapter reached <see cref="Environment.Exit(int)"/> with a second <c>VM_Dispose</c> waiting, without a
    /// timeout, for an answer that never came; Mono's exit, which suspends every other thread first, then retried
    /// forever (the busy loop; see <c>Program.Exit</c> for how the adapter exits now). Now the detach is sent once and
    /// awaited, bounded, so the program is released before the adapter exits; its end is not reported as <c>exited</c>.
    /// <see cref="DebuggerSession.Dispose"/> is not a detach: it calls <c>VM.Exit</c>, which ends the debuggee.
    /// </remarks>
    private void EndSession(bool terminate)
    {
        if (_ended)
        {
            return;
        }

        _ended = true;
        var s = _session;
        if (s is not null && _started && !_exitedSent)
        {
            try
            {
                if (terminate)
                {
                    s.Exit();
                }
                else
                {
                    _detached = true;
                    s.Detach();
                    if (!_targetGone.Wait(DetachTimeoutMs))
                    {
                        _log.Write("detach: the debuggee's agent did not let go within " + DetachTimeoutMs.ToString(CultureInfo.InvariantCulture) + " ms");
                    }
                }
            }
#pragma warning disable CA1031 // Ending the session must not fail the disconnect.
            catch (Exception e)
#pragma warning restore CA1031
            {
                _log.Write((terminate ? "exit: " : "detach: ") + e.Message);
            }
        }

        if (terminate && _process is not null)
        {
            if (!_process.WaitForExit(2000))
            {
                _process.Kill();
                _process.WaitForExit(2000);
            }

            if (!_exitedSent && _process.ExitCode is int code)
            {
                SendExited(code);
            }
        }

        _stopped = false;
        ClearStop();
    }

    private global::Mono.Debugger.Soft.ITargetProcess StartTarget(ProcessStartInfo info)
    {
        var p = new TargetProcess(info);
        _process = p;
        _log.Write("started " + info.FileName + " " + info.Arguments + " as process " + p.Id.ToString(CultureInfo.InvariantCulture));
        var name = _program ?? info.FileName;
        _d.Post(() => _d.SendEvent("process", new JObject
        {
            ["name"] = name,
            ["systemProcessId"] = p.Id,
            ["isLocalProcess"] = true,
            ["startMethod"] = "launch",
        }));
        return p;
    }

    private DebuggerSessionOptions SessionOptions(bool justMyCode) => new()
    {
        EvaluationOptions = _evaluation,
        ProjectAssembliesOnly = justMyCode,
        StepOverPropertiesAndOperators = true,
    };

    /// <summary>The mono that runs the program: the argument (a path, or a name on PATH), else the one running this adapter.</summary>
    private static string ResolveMono(string? requested)
    {
        if (requested is null)
        {
            var self = Process.GetCurrentProcess().MainModule?.FileName;
            if (!string.IsNullOrEmpty(self) && File.Exists(self))
            {
                return self!;
            }

            requested = "mono";
        }

        if (requested.IndexOf(Path.DirectorySeparatorChar) >= 0)
        {
            var full = Path.GetFullPath(requested);
            return File.Exists(full) ? full : throw new DapException("runtimeExecutable " + full + " does not exist");
        }

        foreach (var dir in (Environment.GetEnvironmentVariable("PATH") ?? string.Empty).Split(Path.PathSeparator))
        {
            if (dir.Length > 0 && File.Exists(Path.Combine(dir, requested)))
            {
                return Path.Combine(dir, requested);
            }
        }

        throw new DapException("runtimeExecutable " + requested + " is not on PATH");
    }

    /// <summary>A one-shot breakpoint on the program's entry point (stopAtEntry), or null when it has none.</summary>
    private FunctionBreakpoint? EntryBreakpoint(string program)
    {
        try
        {
            var assembly = global::Mono.Cecil.AssemblyDefinition.ReadAssembly(program);
            var entry = assembly.EntryPoint;
            if (entry is null)
            {
                return null;
            }

            return new FunctionBreakpoint(entry.DeclaringType.FullName.Replace('/', '+') + "." + entry.Name, "C#");
        }
        catch (Exception e) when (e is IOException or BadImageFormatException or InvalidOperationException)
        {
            _log.Write("stopAtEntry: cannot read the entry point of " + program + ": " + e.Message);
            return null;
        }
    }

    // ----- Breakpoints -----

    private JToken? SetBreakpoints(JObject args)
    {
        var s = Session;
        var source = args["source"] as JObject ?? throw new DapException("setBreakpoints needs `source`");
        var path = Json.String(source, "path");
        if (string.IsNullOrEmpty(path))
        {
            throw new DapException("setBreakpoints needs `source.path` (sources by reference are not supported)");
        }

        if (_sourceBreakpoints.TryGetValue(path!, out var old))
        {
            foreach (var bp in old)
            {
                s.Breakpoints.Remove(bp);
                _breakpointIds.Remove(bp);
            }
        }

        var list = new List<Breakpoint>();
        var answers = new JArray();
        foreach (var b in (args["breakpoints"] as JArray ?? new JArray()).OfType<JObject>())
        {
            var line = (Json.Int(b, "line") ?? throw new DapException("a breakpoint needs `line`")) + 1 - _lineBase;
            var bp = new Breakpoint(path, line);
            var problem = Configure(bp, b);
            s.Breakpoints.Add(bp);
            list.Add(bp);
            var id = _nextBreakpointId++;
            _breakpointIds[bp] = id;
            answers.Add(BreakpointJson(bp, id, problem));
        }

        _sourceBreakpoints[path!] = list;
        return new JObject { ["breakpoints"] = answers };
    }

    private JToken? SetFunctionBreakpoints(JObject args)
    {
        var s = Session;
        foreach (var bp in _functionBreakpoints)
        {
            s.Breakpoints.Remove(bp);
            _breakpointIds.Remove(bp);
        }

        _functionBreakpoints.Clear();
        _functionSpecs.Clear();
        var answers = new JArray();
        foreach (var b in (args["breakpoints"] as JArray ?? new JArray()).OfType<JObject>())
        {
            var name = (Json.String(b, "name") ?? string.Empty).Trim();
            if (name.Length == 0)
            {
                throw new DapException("a function breakpoint needs `name`");
            }

            var paren = name.IndexOf('(');
            var bp = new FunctionBreakpoint(paren > 0 ? name.Substring(0, paren).Trim() : name, "C#");
            string? problem = null;
            if (paren > 0)
            {
                var close = name.LastIndexOf(')');
                if (close > paren && FunctionBreakpoint.TryParseParameters(name, paren + 1, close, out var types))
                {
                    bp.ParamTypes = types;
                }
                else
                {
                    problem = "cannot read the parameter list of " + name;
                }
            }

            // Mono.Debugging 2017 binds a function breakpoint to every method of the type (its name filter is broken),
            // so the adapter filters the hits by method and applies the condition, hit count and log message itself.
            var spec = new FunctionSpec(bp.FunctionName)
            {
                Condition = Json.String(b, "condition") is { Length: > 0 } c ? c : null,
                LogMessage = Json.String(b, "logMessage") is { Length: > 0 } m ? m : null,
            };
            if (Json.String(b, "hitCondition") is { Length: > 0 } hit)
            {
                if (HitCondition.TryParse(hit, out var h))
                {
                    spec.Hit = h;
                }
                else
                {
                    problem ??= "the hit condition `" + hit + "` is not N, >=N or %N; the breakpoint breaks on every hit";
                }
            }

            s.Breakpoints.Add(bp);
            _functionBreakpoints.Add(bp);
            _functionSpecs[bp] = spec;
            var id = _nextBreakpointId++;
            _breakpointIds[bp] = id;
            answers.Add(BreakpointJson(bp, id, problem));
        }

        return new JObject { ["breakpoints"] = answers };
    }

    /// <summary>Apply a source or function breakpoint's condition, hit condition and log message; a problem, or null.</summary>
    private static string? Configure(Breakpoint bp, JObject b)
    {
        string? problem = null;
        if (Json.String(b, "condition") is { Length: > 0 } condition)
        {
            bp.ConditionExpression = condition;
        }

        if (Json.String(b, "hitCondition") is { Length: > 0 } hit)
        {
            if (HitCondition.TryParse(hit, out var h))
            {
                bp.HitCountMode = h.Kind switch
                {
                    HitConditionKind.EqualTo => HitCountMode.EqualTo,
                    HitConditionKind.GreaterThanOrEqualTo => HitCountMode.GreaterThanOrEqualTo,
                    HitConditionKind.GreaterThan => HitCountMode.GreaterThan,
                    HitConditionKind.LessThan => HitCountMode.LessThan,
                    HitConditionKind.LessThanOrEqualTo => HitCountMode.LessThanOrEqualTo,
                    _ => HitCountMode.MultipleOf,
                };
                bp.HitCount = h.Count;
            }
            else
            {
                problem = "the hit condition `" + hit + "` is not N, >=N or %N; the breakpoint breaks on every hit";
            }
        }

        if (Json.String(b, "logMessage") is { Length: > 0 } message)
        {
            bp.HitAction = HitAction.PrintExpression;
            bp.TraceExpression = LogMessage.ToTraceExpression(message);
        }
        else
        {
            bp.HitAction = HitAction.Break;
        }

        return problem;
    }

    private JObject BreakpointJson(BreakEvent be, int id, string? problem = null)
    {
        var status = _session is null ? BreakEventStatus.Disconnected : be.GetStatus(_session);
        var o = new JObject { ["id"] = id, ["verified"] = status == BreakEventStatus.Bound && problem is null };
        if (be is Breakpoint bp && be is not FunctionBreakpoint)
        {
            o["line"] = bp.Line - 1 + _lineBase;
            o["source"] = new JObject { ["name"] = Path.GetFileName(bp.FileName), ["path"] = bp.FileName };
        }

        if (problem is not null)
        {
            o["message"] = problem;
        }
        else if (status != BreakEventStatus.Bound)
        {
            var message = _session is null ? null : be.GetStatusMessage(_session);
            o["message"] = string.IsNullOrEmpty(message)
                ? (status == BreakEventStatus.Invalid || status == BreakEventStatus.BindError
                    ? "The breakpoint cannot be bound."
                    : "The breakpoint will bind when its code loads.")
                : message;
        }

        return o;
    }

    private void OnBreakEventStatus(BreakEvent be)
    {
        if (!_breakpointIds.TryGetValue(be, out var id))
        {
            return;
        }

        _d.SendEvent("breakpoint", new JObject { ["reason"] = "changed", ["breakpoint"] = BreakpointJson(be, id) });
    }

    private JToken? SetExceptionBreakpoints(JObject args)
    {
        var s = Session;
        var settings = ExceptionSettings.Parse(args);
        foreach (var cp in _catchpoints)
        {
            s.Breakpoints.Remove(cp);
        }

        _catchpoints.Clear();
        foreach (var type in settings.ThrownTypes)
        {
            var cp = new Catchpoint(type, includeSubclasses: true);
            s.Breakpoints.Add(cp);
            _catchpoints.Add(cp);
        }

        _exceptions = settings;
        var filters = Json.Strings(args, "filters").Count + ((args["filterOptions"] as JArray)?.Count ?? 0);
        var answers = new JArray();
        for (var i = 0; i < filters; i++)
        {
            answers.Add(new JObject { ["verified"] = true });
        }

        return new JObject { ["breakpoints"] = answers };
    }

    // ----- Stops -----

    private void OnTargetEvent(TargetEventArgs e)
    {
        switch (e.Type)
        {
            case TargetEventType.ThreadStarted when e.Thread is not null:
                _d.SendEvent("thread", new JObject { ["reason"] = "started", ["threadId"] = e.Thread.Id });
                break;
            case TargetEventType.ThreadStopped when e.Thread is not null:
                _d.SendEvent("thread", new JObject { ["reason"] = "exited", ["threadId"] = e.Thread.Id });
                break;
            case TargetEventType.TargetExited:
                _stopped = false;
                ClearStop();
                if (_detached)
                {
                    // The detach's own end: the debuggee runs on, so it has no exit code to report.
                    SendTerminated();
                    break;
                }

                SendExited(e.ExitCode ?? _process?.ExitCode ?? 0);
                SendTerminated();
                break;
            case TargetEventType.TargetHitBreakpoint:
                OnBreakpointHit(e);
                break;
            case TargetEventType.TargetStopped:
                if (!_pausing && StepInReturnedToItsLine(e))
                {
                    _session?.StepLine();
                    break;
                }

                Stopped(e, _pausing ? "pause" : "step");
                break;
            case TargetEventType.TargetInterrupted:
            case TargetEventType.TargetSignaled:
                Stopped(e, "pause");
                break;
            case TargetEventType.ExceptionThrown:
                // Just My Code: a first-chance exception thrown in external code (the runtime's own, handled there) does
                // not stop, as Visual Studio's "break when thrown" with Just My Code on.
                if (_justMyCode && e.Backtrace is { FrameCount: > 0 } bt && IsExternal(bt.GetFrame(0)))
                {
                    _session?.Continue();
                    break;
                }

                Stopped(e, "exception");
                break;
            case TargetEventType.UnhandledException:
                if (!_exceptions.BreakWhenUserUnhandled || !UnhandledTypeMatches(e))
                {
                    _session?.Continue();
                    break;
                }

                _unhandledStop = true;
                Stopped(e, "exception");
                break;
            default:
                break;
        }
    }

    private bool UnhandledTypeMatches(TargetEventArgs e)
    {
        if (_exceptions.UnhandledTypes.Count == 0)
        {
            return true;
        }

        var type = ExceptionOf(e.Backtrace)?.Type;
        return type is not null && _exceptions.UnhandledTypes.Contains(type, StringComparer.Ordinal);
    }

    private void OnBreakpointHit(TargetEventArgs e)
    {
        var be = e.BreakEvent;
        if (be is FunctionBreakpoint fb && !FunctionHitBreaks(fb, e))
        {
            Session.Continue();
            return;
        }

        if (be is not null && ReferenceEquals(be, _entryBreakpoint))
        {
            Session.Breakpoints.Remove(be);
            _entryBreakpoint = null;
            Stopped(e, "entry");
            return;
        }

        var reason = be is FunctionBreakpoint ? "function breakpoint" : be is Catchpoint ? "exception" : "breakpoint";
        var ids = new JArray();
        if (be is not null && _breakpointIds.TryGetValue(be, out var id))
        {
            ids.Add(id);
        }

        Stopped(e, reason, ids);
    }

    /// <summary>
    /// Whether a function breakpoint's hit stops: the method is the one named (Mono.Debugging also binds the type's other
    /// methods), its condition holds and its hit count is reached; a log point writes its message and goes on.
    /// </summary>
    private bool FunctionHitBreaks(FunctionBreakpoint bp, TargetEventArgs e)
    {
        var frame = e.Backtrace is { FrameCount: > 0 } bt ? bt.GetFrame(0) : null;
        if (frame is null)
        {
            return true;
        }

        var method = MethodName(frame);
        if (!string.Equals(method, bp.FunctionName, StringComparison.Ordinal))
        {
            return false;
        }

        if (!_functionSpecs.TryGetValue(bp, out var spec))
        {
            return true;
        }

        if (spec.Condition is not null)
        {
            string? value;
            try
            {
                value = EvaluateValue(frame, spec.Condition, DefaultEvaluationTimeoutMs).Value;
            }
            catch (DapException x)
            {
                Output("console", "The condition `" + spec.Condition + "` of the function breakpoint " + spec.Name + " failed: " + x.Message + "\n");
                return true;
            }

            if (!string.Equals(value, "true", StringComparison.OrdinalIgnoreCase))
            {
                return false;
            }
        }

        spec.Hits++;
        if (spec.Hit is HitCondition h && !h.BreaksOn(spec.Hits))
        {
            return false;
        }

        if (spec.LogMessage is not null)
        {
            var text = LogMessage.Interpolate(spec.LogMessage, expression =>
            {
                try
                {
                    return Display(EvaluateValue(frame, expression, DefaultEvaluationTimeoutMs));
                }
                catch (DapException x)
                {
                    return "{" + x.Message + "}";
                }
            });
            Output("console", text + "\n");
            return false;
        }

        return true;
    }

    private static bool IsExternal(global::Mono.Debugging.Client.StackFrame f) =>
        f.IsExternalCode || !f.HasDebugInfo || string.IsNullOrEmpty(f.SourceLocation.FileName);

    /// <summary><c>Namespace.Type.Method</c> of a frame.</summary>
    private static string MethodName(global::Mono.Debugging.Client.StackFrame f)
    {
        var method = f.SourceLocation.MethodName ?? string.Empty;
        var type = f.FullTypeName;
        return string.IsNullOrEmpty(type) || method.StartsWith(type + ".", StringComparison.Ordinal)
            ? method
            : type + "." + LastSegment(method);
    }

    /// <summary>Where a step in started: the thread, its stack depth, and the top frame's method and line.</summary>
    private static (long Thread, int Depth, string Method, string File, int Line)? OriginOf(long thread, Backtrace? backtrace)
    {
        if (backtrace is null || backtrace.FrameCount == 0)
        {
            return null;
        }

        var top = backtrace.GetFrame(0).SourceLocation;
        return (thread, backtrace.FrameCount, top.MethodName ?? string.Empty, top.FileName ?? string.Empty, top.Line);
    }

    /// <summary>
    /// Step in with Just My Code: Mono steps out of a method of an assembly without symbols (string concatenation,
    /// <c>Console.WriteLine</c>) and stops back on the calling line, part way through it. Visual Studio goes on to the
    /// next line, so a step in that comes back to the frame and the line it started on steps in again (Mono's line
    /// steps never stop twice on one line of one frame otherwise). Bounded, so a line that calls external code in a
    /// loop still stops.
    /// </summary>
    private bool StepInReturnedToItsLine(TargetEventArgs e)
    {
        var from = _stepInFrom;
        if (from is null || e.Thread is null || e.Thread.Id != from.Value.Thread || _stepInRetries >= MaxStepInRetries)
        {
            return false;
        }

        var here = OriginOf(e.Thread.Id, e.Backtrace);
        if (here is null || here.Value != from.Value)
        {
            return false;
        }

        _stepInRetries++;
        return true;
    }

    private const int MaxStepInRetries = 32;

    private JToken? StepIn(JObject args)
    {
        (long Thread, int Depth, string Method, string File, int Line)? from = null;
        if (_stopped)
        {
            var thread = Json.Int(args, "threadId") is int id && _stopThread is { } stopped && stopped.Id != id
                ? AllThreads().FirstOrDefault(t => t.Id == id)
                : _stopThread;
            if (thread is not null)
            {
                from = OriginOf(thread.Id, _backtraces.TryGetValue(thread.Id, out var bt) ? bt : thread.Backtrace);
            }
        }

        var answer = Resume(args, s => s.StepLine());
        _stepInFrom = _justMyCode ? from : null;
        _stepInRetries = 0;
        return answer;
    }

    private void Stopped(TargetEventArgs e, string reason, JArray? hitIds = null)
    {
        _stepInFrom = null;
        ClearStop();
        _stopped = true;
        _pausing = false;
        _stopThread = e.Thread ?? _session?.ActiveThread;
        if (_stopThread is not null && e.Backtrace is not null)
        {
            _backtraces[_stopThread.Id] = e.Backtrace;
        }

        var body = new JObject
        {
            ["reason"] = reason,
            ["threadId"] = _stopThread?.Id ?? 0,
            ["allThreadsStopped"] = true,
        };
        if (hitIds is { Count: > 0 })
        {
            body["hitBreakpointIds"] = hitIds;
        }

        if (reason == "exception")
        {
            var ex = ExceptionOf(e.Backtrace);
            if (ex is not null)
            {
                body["description"] = (_unhandledStop ? "Unhandled exception: " : "Exception thrown: ") + ex.Type;
                body["text"] = ex.Message;
            }
        }

        _d.SendEvent("stopped", body);
    }

    private ExceptionInfo? ExceptionOf(Backtrace? backtrace)
    {
        if (backtrace is null || backtrace.FrameCount == 0)
        {
            return null;
        }

        try
        {
            return backtrace.GetFrame(0).GetException(Options(DefaultEvaluationTimeoutMs));
        }
#pragma warning disable CA1031 // A stop without readable exception details still stops.
        catch (Exception x)
#pragma warning restore CA1031
        {
            _log.Write("exception details: " + x.Message);
            return null;
        }
    }

    /// <summary>Forget everything that names the last stop: its frames, values and threads.</summary>
    private void ClearStop()
    {
        _variables.Clear();
        _frames.Clear();
        _backtraces.Clear();
        _threads = null;
        _stopThread = null;
        _unhandledStop = false;
        _evaluator?.ForgetMisses();
    }

    private JToken? Resume(JObject args, Action<SoftDebuggerSession> resume)
    {
        var s = Session;
        if (!_stopped)
        {
            throw new DapException("the debuggee is not stopped");
        }

        // Steps move the thread the client names (Mono.Debugging steps its active thread).
        if (Json.Int(args, "threadId") is int threadId && _stopThread is { } stopped && stopped.Id != threadId)
        {
            AllThreads().FirstOrDefault(t => t.Id == threadId)?.SetActive();
        }

        _stopped = false;
        _stepInFrom = null;
        ClearStop();
        resume(s);
        _d.Post(() => _d.SendEvent("continued", new JObject { ["threadId"] = Json.Int(args, "threadId") ?? 0, ["allThreadsContinued"] = true }));
        return new JObject { ["allThreadsContinued"] = true };
    }

    private JToken? Pause(JObject args)
    {
        var s = Session;
        if (_stopped || !_started)
        {
            return null;
        }

        _pausing = true;
        s.Stop();
        return null;
    }

    // ----- Threads, frames and values -----

    private ThreadInfo[] AllThreads()
    {
        if (_threads is not null)
        {
            return _threads;
        }

        var processes = _session?.GetProcesses();
        var threads = processes is { Length: > 0 } ? processes[0].GetThreads() : Array.Empty<ThreadInfo>();
        if (_stopped)
        {
            _threads = threads;
        }

        return threads;
    }

    private JToken? Threads(JObject args)
    {
        var list = new JArray();
        if (_session is not null && _started && !_exitedSent)
        {
            try
            {
                foreach (var t in AllThreads())
                {
                    list.Add(new JObject
                    {
                        ["id"] = t.Id,
                        ["name"] = string.IsNullOrEmpty(t.Name) ? "Thread " + t.Id.ToString(CultureInfo.InvariantCulture) : t.Name,
                    });
                }
            }
            catch (Exception e) when (e is InvalidOperationException or IOException or NotSupportedException)
            {
                _log.Write("threads: " + e.Message);
            }
        }

        return new JObject { ["threads"] = list };
    }

    private void EnsureStopped()
    {
        if (!_stopped)
        {
            throw new DapException("the debuggee is running: this needs it stopped");
        }
    }

    private Backtrace BacktraceOf(long threadId)
    {
        if (_backtraces.TryGetValue(threadId, out var bt))
        {
            return bt;
        }

        var thread = AllThreads().FirstOrDefault(t => t.Id == threadId)
            ?? throw new DapException("no thread " + threadId.ToString(CultureInfo.InvariantCulture));
        bt = thread.Backtrace;
        _backtraces[threadId] = bt;
        return bt;
    }

    private JToken? StackTrace(JObject args)
    {
        EnsureStopped();
        var threadId = (long?)args["threadId"] ?? _stopThread?.Id ?? throw new DapException("stackTrace needs `threadId`");
        var bt = BacktraceOf(threadId);
        var total = bt.FrameCount;
        var (start, count) = Paging.Window(total, Json.Int(args, "startFrame"), Json.Int(args, "levels"));
        var frames = new JArray();
        for (var i = start; i < start + count; i++)
        {
            var f = bt.GetFrame(i);
            var id = _frames.Add(new FrameRef(threadId, i, f));
            frames.Add(FrameJson(f, id));
        }

        return new JObject { ["stackFrames"] = frames, ["totalFrames"] = total };
    }

    private JObject FrameJson(global::Mono.Debugging.Client.StackFrame f, int id)
    {
        var loc = f.SourceLocation;
        var o = new JObject
        {
            ["id"] = id,
            ["name"] = FrameName(f),
            ["line"] = 0,
            ["column"] = 0,
        };
        var hasSource = f.HasDebugInfo && !string.IsNullOrEmpty(loc.FileName) && loc.Line > 0;
        if (hasSource)
        {
            o["source"] = new JObject { ["name"] = Path.GetFileName(loc.FileName), ["path"] = loc.FileName };
            o["line"] = loc.Line - 1 + _lineBase;
            o["column"] = Math.Max(loc.Column, 1) - 1 + _columnBase;
            if (loc.EndLine > 0)
            {
                o["endLine"] = loc.EndLine - 1 + _lineBase;
                o["endColumn"] = Math.Max(loc.EndColumn, 1) - 1 + _columnBase;
            }
        }

        if (f.IsExternalCode || !hasSource)
        {
            o["presentationHint"] = "subtle";
        }

        if (!string.IsNullOrEmpty(f.FullModuleName))
        {
            o["moduleId"] = Path.GetFileName(f.FullModuleName);
        }

        return o;
    }

    /// <summary>The method as the Call Stack shows it: <c>Namespace.Type.Method(int, int)</c>.</summary>
    private static string FrameName(global::Mono.Debugging.Client.StackFrame f)
    {
        var name = MethodName(f);
        var text = f.FullStackframeText;
        var paren = text?.IndexOf('(') ?? -1;
        if (paren >= 0 && text!.EndsWith(")", StringComparison.Ordinal))
        {
            return name + text.Substring(paren);
        }

        return name.Contains('(') ? name : name + "()";
    }

    private static string LastSegment(string method)
    {
        var dot = method.LastIndexOf('.');
        return dot >= 0 ? method.Substring(dot + 1) : method;
    }

    private FrameRef FrameOf(int? frameId)
    {
        EnsureStopped();
        if (frameId is int id)
        {
            return _frames.Get(id) ?? throw new DapException("unknown frame " + id.ToString(CultureInfo.InvariantCulture) + " (the debuggee moved since)");
        }

        var thread = _stopThread ?? throw new DapException("no stopped thread");
        var bt = BacktraceOf(thread.Id);
        if (bt.FrameCount == 0)
        {
            throw new DapException("the stopped thread has no frames");
        }

        return new FrameRef(thread.Id, 0, bt.GetFrame(0));
    }

    private JToken? Scopes(JObject args)
    {
        var frame = FrameOf(Json.Int(args, "frameId") ?? throw new DapException("scopes needs `frameId`"));
        var reference = _variables.Add(new FrameScope(frame.Frame));
        return new JObject
        {
            ["scopes"] = new JArray
            {
                new JObject
                {
                    ["name"] = "Locals",
                    ["presentationHint"] = "locals",
                    ["variablesReference"] = reference,
                    ["expensive"] = false,
                },
            },
        };
    }

    private EvaluationOptions Options(int timeoutMs)
    {
        var o = _evaluation.Clone();
        o.EvaluationTimeout = timeoutMs;
        o.MemberEvaluationTimeout = timeoutMs;
        return o;
    }

    /// <summary>Wait for values still being evaluated, within the timeout.</summary>
    private static void Settle(IEnumerable<ObjectValue> values, int timeoutMs)
    {
        var deadline = Stopwatch.StartNew();
        foreach (var v in values)
        {
            if (v is not null && v.IsEvaluating)
            {
                var left = (int)Math.Max(0, timeoutMs - deadline.ElapsedMilliseconds);
                v.WaitHandle.WaitOne(left);
            }
        }
    }

    private List<ObjectValue> FrameValues(global::Mono.Debugging.Client.StackFrame frame)
    {
        var options = Options(DefaultEvaluationTimeoutMs);
        var values = new List<ObjectValue>();
        var self = frame.GetThisReference(options);
        if (self is not null)
        {
            values.Add(self);
        }

        values.AddRange(frame.GetParameters(options));
        values.AddRange(frame.GetLocalVariables(options));
        Settle(values, DefaultEvaluationTimeoutMs);
        return values;
    }

    private JToken? Variables(JObject args)
    {
        EnsureStopped();
        var reference = Json.Int(args, "variablesReference") ?? throw new DapException("variables needs `variablesReference`");
        var container = _variables.Get(reference)
            ?? throw new DapException("unknown variables reference " + reference.ToString(CultureInfo.InvariantCulture) + " (the debuggee moved since)");
        var start = Json.Int(args, "start");
        var count = Json.Int(args, "count");
        var options = Options(DefaultEvaluationTimeoutMs);
        IReadOnlyList<ObjectValue> page;
        string? parent = null;
        switch (container)
        {
            case FrameScope scope:
                page = Paging.Page(FrameValues(scope.Frame), start, count);
                break;
            case ValueRef value when ArrayLength(value.Value) is int length:
            {
                parent = value.EvaluateName;
                var (s, c) = Paging.Window(length, start, count);
                page = c == 0 ? Array.Empty<ObjectValue>() : value.Value.GetRangeOfChildren(s, c, options);
                Settle(page, DefaultEvaluationTimeoutMs);
                break;
            }

            case ValueRef value:
            {
                parent = value.EvaluateName;
                var all = value.Value.GetAllChildren(options);
                Settle(all, DefaultEvaluationTimeoutMs);
                page = Paging.Page(all, start, count);
                break;
            }

            default:
                throw new DapException("unknown variables reference " + reference.ToString(CultureInfo.InvariantCulture));
        }

        var list = new JArray();
        foreach (var v in page)
        {
            list.Add(VariableJson(v, ChildName(parent, v)));
        }

        return new JObject { ["variables"] = list };
    }

    /// <summary>An expression for a child value, or null when it has none (a group of members).</summary>
    private static string? ChildName(string? parent, ObjectValue v)
    {
        var name = v.Name ?? string.Empty;
        // Groups of members and the ranges a long array is split in (`[0..99]`) have no expression.
        if (v.HasFlag(ObjectValueFlags.Group) || name.Length == 0 || name.Contains(".."))
        {
            return null;
        }

        if (parent is null)
        {
            return name;
        }

        return name.StartsWith("[", StringComparison.Ordinal) ? parent + name : parent + "." + name;
    }

    private JObject VariableJson(ObjectValue v, string? evaluateName)
    {
        var o = new JObject
        {
            ["name"] = v.Name ?? string.Empty,
            ["value"] = Display(v),
            ["type"] = v.TypeName ?? string.Empty,
            ["variablesReference"] = Reference(v, evaluateName),
        };
        if (ArrayLength(v) is int length)
        {
            o["indexedVariables"] = length;
        }

        if (evaluateName is not null)
        {
            o["evaluateName"] = evaluateName;
        }

        return o;
    }

    /// <summary>Mono.Debugging lists an array's elements as its children up to this length, and groups them in ranges above it.</summary>
    private const int MaxListedElements = 150;

    /// <summary>
    /// The length of a one-dimensional array whose children are its elements (so <c>start</c> and <c>count</c> page them),
    /// read from Mono.Debugging's display string (<c>{string[25]}</c>); null otherwise.
    /// </summary>
    private static int? ArrayLength(ObjectValue v)
    {
        var type = v.TypeName ?? string.Empty;
        var value = v.Value ?? string.Empty;
        if (!type.EndsWith("[]", StringComparison.Ordinal) || !value.EndsWith("]}", StringComparison.Ordinal))
        {
            return null;
        }

        var open = value.LastIndexOf('[');
        return open > 0 && int.TryParse(value.Substring(open + 1, value.Length - open - 3), NumberStyles.None, CultureInfo.InvariantCulture, out var n) && n <= MaxListedElements
            ? n
            : null;
    }

    private int Reference(ObjectValue v, string? evaluateName) =>
        v.HasChildren && !v.IsNull && !v.IsError ? _variables.Add(new ValueRef(v, evaluateName)) : 0;

    private static string Display(ObjectValue v)
    {
        if (v.IsEvaluating)
        {
            return "(evaluating timed out)";
        }

        return v.DisplayValue ?? v.Value ?? string.Empty;
    }

    private ObjectValue EvaluateValue(global::Mono.Debugging.Client.StackFrame frame, string expression, int timeoutMs)
    {
        var options = Options(timeoutMs);
        var clock = Stopwatch.StartNew();
        var v = frame.GetExpressionValue(expression, options);
        if (v.IsEvaluating && !v.WaitHandle.WaitOne((int)Math.Max(0, timeoutMs - clock.ElapsedMilliseconds)))
        {
            Session.CancelAsyncEvaluations();
            throw new DapException("the evaluation of `" + expression + "` did not finish within its timeout of " + timeoutMs.ToString(CultureInfo.InvariantCulture) + " ms and was aborted");
        }

        if (v.IsError || v.IsUnknown || v.IsNotSupported || v.IsImplicitNotSupported)
        {
            if (clock.ElapsedMilliseconds >= timeoutMs)
            {
                throw new DapException("the evaluation of `" + expression + "` did not finish within its timeout of " + timeoutMs.ToString(CultureInfo.InvariantCulture) + " ms and was aborted (" + v.Value + ")");
            }

            var message = v.Value;
            throw new DapException(string.IsNullOrEmpty(message) ? "cannot evaluate `" + expression + "`" : message);
        }

        return v;
    }

    private JToken? Evaluate(JObject args)
    {
        var expression = (Json.String(args, "expression") ?? string.Empty).Trim();
        if (expression.Length == 0)
        {
            throw new DapException("evaluate needs `expression`");
        }

        var timeout = Json.Int(args, "timeout") ?? DefaultEvaluationTimeoutMs;
        if (timeout <= 0)
        {
            throw new DapException("`timeout` must be positive");
        }

        if (!_stopped)
        {
            throw new DapException("cannot evaluate `" + expression + "` while the debuggee runs");
        }

        var frame = FrameOf(Json.Int(args, "frameId"));
        var v = EvaluateValue(frame.Frame, expression, timeout);
        var o = new JObject
        {
            ["result"] = Display(v),
            ["type"] = v.TypeName ?? string.Empty,
            ["variablesReference"] = Reference(v, expression),
        };
        if (ArrayLength(v) is int length)
        {
            o["indexedVariables"] = length;
        }

        return o;
    }

    private JToken? SetVariable(JObject args)
    {
        EnsureStopped();
        var reference = Json.Int(args, "variablesReference") ?? throw new DapException("setVariable needs `variablesReference`");
        var name = Json.String(args, "name") ?? throw new DapException("setVariable needs `name`");
        var value = Json.String(args, "value") ?? throw new DapException("setVariable needs `value`");
        var container = _variables.Get(reference)
            ?? throw new DapException("unknown variables reference " + reference.ToString(CultureInfo.InvariantCulture) + " (the debuggee moved since)");
        var options = Options(DefaultEvaluationTimeoutMs);
        IEnumerable<ObjectValue> candidates = container switch
        {
            FrameScope scope => FrameValues(scope.Frame),
            ValueRef v => v.Value.GetAllChildren(options),
            _ => Array.Empty<ObjectValue>(),
        };
        var target = candidates.FirstOrDefault(c => c.Name == name)
            ?? throw new DapException("no variable `" + name + "` in this scope");
        if (target.IsReadOnly)
        {
            throw new DapException("`" + name + "` is read-only");
        }

        try
        {
            target.SetValue(value, options);
        }
#pragma warning disable CA1031 // The evaluator's failure is the request's answer.
        catch (Exception e)
#pragma warning restore CA1031
        {
            throw new DapException("cannot set `" + name + "` to `" + value + "`: " + e.Message, e);
        }

        if (target.IsError)
        {
            throw new DapException("cannot set `" + name + "` to `" + value + "`: " + target.Value);
        }

        return new JObject
        {
            ["value"] = Display(target),
            ["type"] = target.TypeName ?? string.Empty,
            ["variablesReference"] = Reference(target, null),
        };
    }

    private JToken? ExceptionInfoRequest(JObject args)
    {
        EnsureStopped();
        var threadId = (long?)args["threadId"] ?? _stopThread?.Id ?? 0;
        var ex = ExceptionOf(BacktraceOf(threadId)) ?? throw new DapException("thread " + threadId.ToString(CultureInfo.InvariantCulture) + " has no exception");
        return new JObject
        {
            ["exceptionId"] = ex.Type,
            ["description"] = ex.Message,
            ["breakMode"] = _unhandledStop ? "userUnhandled" : "always",
            ["details"] = ExceptionDetails(ex, 0),
        };
    }

    private static JObject ExceptionDetails(ExceptionInfo ex, int depth)
    {
        var o = new JObject
        {
            ["message"] = ex.Message,
            ["typeName"] = ex.Type,
            ["fullTypeName"] = ex.Type,
        };
        var stack = ex.StackTrace;
        if (stack is { Length: > 0 })
        {
            o["stackTrace"] = string.Join("\n", stack.Select(f => "   at " + f.DisplayText + (string.IsNullOrEmpty(f.File) ? string.Empty : " in " + f.File + ":line " + f.Line.ToString(CultureInfo.InvariantCulture))));
        }

        if (ex.InnerException is { } inner && depth < 8)
        {
            o["innerException"] = new JArray(ExceptionDetails(inner, depth + 1));
        }

        return o;
    }

    // ----- Output and the end -----

    private void Output(string category, string text)
    {
        if (string.IsNullOrEmpty(text))
        {
            return;
        }

        _d.SendEvent("output", new JObject { ["category"] = category, ["output"] = text });
    }

    private void SendExited(int code)
    {
        if (_exitedSent)
        {
            return;
        }

        _exitedSent = true;
        _d.SendEvent("exited", new JObject { ["exitCode"] = code });
    }

    private void SendTerminated()
    {
        if (_terminatedSent)
        {
            return;
        }

        _terminatedSent = true;
        _d.SendEvent("terminated");
    }

    /// <summary>What a function breakpoint asks for beyond the method (applied by the adapter, see <see cref="FunctionHitBreaks"/>).</summary>
    private sealed class FunctionSpec
    {
        public FunctionSpec(string name) => Name = name;

        public string Name { get; }

        public string? Condition { get; set; }

        public HitCondition? Hit { get; set; }

        public string? LogMessage { get; set; }

        public int Hits { get; set; }
    }

    private sealed class FrameRef
    {
        public FrameRef(long threadId, int index, global::Mono.Debugging.Client.StackFrame frame)
        {
            ThreadId = threadId;
            Index = index;
            Frame = frame;
        }

        public long ThreadId { get; }

        public int Index { get; }

        public global::Mono.Debugging.Client.StackFrame Frame { get; }
    }

    private sealed class FrameScope
    {
        public FrameScope(global::Mono.Debugging.Client.StackFrame frame) => Frame = frame;

        public global::Mono.Debugging.Client.StackFrame Frame { get; }
    }

    private sealed class ValueRef
    {
        public ValueRef(ObjectValue value, string? evaluateName)
        {
            Value = value;
            EvaluateName = evaluateName;
        }

        public ObjectValue Value { get; }

        public string? EvaluateName { get; }
    }
}
