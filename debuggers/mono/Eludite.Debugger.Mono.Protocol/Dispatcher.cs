using System.Collections.Concurrent;
using Newtonsoft.Json.Linq;

namespace Eludite.Debugger.Mono.Protocol;

/// <summary>A request that fails: the dispatcher answers it with <c>success: false</c> and this message.</summary>
public sealed class DapException : Exception
{
    public DapException(string message)
        : base(message)
    {
    }

    public DapException(string message, Exception inner)
        : base(message, inner)
    {
    }
}

/// <summary>
/// The adapter's side of one DAP connection: it reads requests on a reader thread and runs every request handler,
/// and every piece of work posted with <see cref="Post"/> (the debugger library's callbacks), on one dispatcher
/// thread, in order. Responses and events are written under one lock with increasing sequence numbers, so any
/// thread may send.
/// </summary>
public sealed class Dispatcher : IDisposable
{
    /// <summary>A request handler: the arguments (an empty object when absent) to the response body (null for none).</summary>
    public delegate JToken? Handler(JObject arguments);

    private readonly Stream _output;
    private readonly FrameReader _reader;
    private readonly Action<string> _log;
    private readonly Dictionary<string, Handler> _handlers = new(StringComparer.Ordinal);
    private readonly BlockingCollection<Action> _queue = new();
    private readonly object _writeLock = new();
    private int _seq;
    private volatile bool _stopped;

    /// <param name="input">Where requests arrive.</param>
    /// <param name="output">Where responses and events go; nothing else may write to it.</param>
    /// <param name="log">The adapter's log (stderr), never the output.</param>
    public Dispatcher(Stream input, Stream output, Action<string> log)
    {
        _reader = new FrameReader(input);
        _output = output;
        _log = log;
    }

    /// <summary>Called on the dispatcher thread when the client closes the connection.</summary>
    public event Action? Closed;

    /// <summary>True on the dispatcher thread.</summary>
    public bool IsDispatcherThread => Thread.CurrentThread.ManagedThreadId == DispatcherThreadId;

    private int DispatcherThreadId { get; set; } = -1;

    /// <summary>Handle <paramref name="command"/> with <paramref name="handler"/> (replacing any previous one).</summary>
    public void Register(string command, Handler handler) => _handlers[command] = handler;

    /// <summary>Run <paramref name="work"/> on the dispatcher thread after what is already queued.</summary>
    public void Post(Action work)
    {
        if (_stopped)
        {
            return;
        }

        try
        {
            _queue.Add(work);
        }
        catch (InvalidOperationException)
        {
            // Stopped meanwhile.
        }
    }

    /// <summary>Stop the dispatcher after the work already queued.</summary>
    public void Stop()
    {
        if (_stopped)
        {
            return;
        }

        _stopped = true;
        _queue.CompleteAdding();
    }

    /// <summary>
    /// Read requests on a background thread and run the dispatcher loop on the calling thread until <see cref="Stop"/>
    /// or until the client closes the connection.
    /// </summary>
    public void Run()
    {
        DispatcherThreadId = Thread.CurrentThread.ManagedThreadId;
        var reader = new Thread(ReadLoop) { IsBackground = true, Name = "DAP reader" };
        reader.Start();
        foreach (var work in _queue.GetConsumingEnumerable())
        {
            try
            {
                work();
            }
#pragma warning disable CA1031 // The loop must survive any handler's failure.
            catch (Exception e)
#pragma warning restore CA1031
            {
                _log("dispatcher: " + e);
            }
        }
    }

    private void ReadLoop()
    {
        while (true)
        {
            JObject? message;
            try
            {
                message = _reader.Read();
            }
#pragma warning disable CA1031 // A broken channel ends the session like a closed one.
            catch (Exception e)
#pragma warning restore CA1031
            {
                _log("reader: " + e.Message);
                message = null;
            }

            if (message is null)
            {
                Post(() =>
                {
                    Closed?.Invoke();
                    Stop();
                });
                return;
            }

            Post(() => Dispatch(message));
        }
    }

    /// <summary>Answer one message (on the dispatcher thread; public for tests that drive it without a reader).</summary>
    public void Dispatch(JObject message)
    {
        if ((string?)message["type"] != "request")
        {
            _log("ignored a message that is not a request: " + (string?)message["type"]);
            return;
        }

        var seq = (int?)message["seq"] ?? 0;
        var command = (string?)message["command"] ?? string.Empty;
        if (!_handlers.TryGetValue(command, out var handler))
        {
            SendResponse(seq, command, success: false, body: null, message: "eludite-dbg-mono does not handle the request `" + command + "`");
            return;
        }

        var arguments = message["arguments"] as JObject ?? new JObject();
        JToken? body;
        try
        {
            body = handler(arguments);
        }
        catch (DapException e)
        {
            SendResponse(seq, command, success: false, body: null, message: e.Message);
            return;
        }
#pragma warning disable CA1031 // Any failure is the request's failed answer, never a crash.
        catch (Exception e)
#pragma warning restore CA1031
        {
            _log(command + ": " + e);
            SendResponse(seq, command, success: false, body: null, message: command + " failed: " + e.Message);
            return;
        }

        SendResponse(seq, command, success: true, body: body, message: null);
    }

    /// <summary>Send an event.</summary>
    public void SendEvent(string name, JObject? body = null)
    {
        var message = new JObject { ["type"] = "event", ["event"] = name };
        if (body is not null)
        {
            message["body"] = body;
        }

        Send(message);
    }

    private void SendResponse(int requestSeq, string command, bool success, JToken? body, string? message)
    {
        var response = new JObject
        {
            ["type"] = "response",
            ["request_seq"] = requestSeq,
            ["success"] = success,
            ["command"] = command,
        };
        if (message is not null)
        {
            response["message"] = message;
            if (!success)
            {
                response["body"] = new JObject { ["error"] = new JObject { ["id"] = 1, ["format"] = message, ["showUser"] = false } };
            }
        }

        if (body is not null && body.Type != JTokenType.Null)
        {
            response["body"] = body;
        }

        Send(response);
    }

    private void Send(JObject message)
    {
        lock (_writeLock)
        {
            message["seq"] = ++_seq;
            var bytes = Framing.Encode(message);
            try
            {
                _output.Write(bytes, 0, bytes.Length);
                _output.Flush();
            }
            catch (IOException e)
            {
                _log("write: " + e.Message);
            }
            catch (ObjectDisposedException e)
            {
                _log("write: " + e.Message);
            }
        }
    }

    public void Dispose() => _queue.Dispose();
}
