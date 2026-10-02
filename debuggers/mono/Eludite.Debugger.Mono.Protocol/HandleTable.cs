namespace Eludite.Debugger.Mono.Protocol;

/// <summary>
/// Numbers handed to the client for things that live only while the debuggee is stopped: DAP's variables references
/// and frame ids. A number names one value in one stop; <see cref="Clear"/> runs when the debuggee resumes, so a
/// number from an earlier stop is unknown afterwards (and numbers are never reused within a session, so a stale one
/// cannot name a new value). Used on the dispatcher thread only.
/// </summary>
public sealed class HandleTable<T>
    where T : class
{
    private readonly Dictionary<int, T> _items = new();
    private int _next;

    /// <param name="first">The first number handed out (DAP reserves 0 for "no reference").</param>
    public HandleTable(int first = 1)
    {
        _next = first;
    }

    /// <summary>How many numbers are live.</summary>
    public int Count => _items.Count;

    /// <summary>A new number for <paramref name="item"/>.</summary>
    public int Add(T item)
    {
        var id = _next++;
        _items[id] = item;
        return id;
    }

    /// <summary>The item numbered <paramref name="id"/> in the current stop, or null.</summary>
    public T? Get(int id) => _items.TryGetValue(id, out var item) ? item : null;

    /// <summary>Forget every number (the debuggee resumed).</summary>
    public void Clear() => _items.Clear();
}

/// <summary>DAP's <c>start</c> and <c>count</c> paging (of <c>variables</c>, and <c>startFrame</c> and <c>levels</c> of <c>stackTrace</c>).</summary>
public static class Paging
{
    /// <summary>
    /// The window of <paramref name="total"/> items that <paramref name="start"/> and <paramref name="count"/> ask for:
    /// a missing or negative start is 0, a missing or non-positive count means to the end.
    /// </summary>
    public static (int Start, int Count) Window(int total, int? start, int? count)
    {
        var s = Math.Min(Math.Max(start ?? 0, 0), total);
        var c = count is int n && n > 0 ? Math.Min(n, total - s) : total - s;
        return (s, c);
    }

    /// <summary>The items of <paramref name="all"/> in the window.</summary>
    public static IReadOnlyList<T> Page<T>(IReadOnlyList<T> all, int? start, int? count)
    {
        var (s, c) = Window(all.Count, start, count);
        if (s == 0 && c == all.Count)
        {
            return all;
        }

        var page = new List<T>(c);
        for (var i = s; i < s + c; i++)
        {
            page.Add(all[i]);
        }

        return page;
    }
}
